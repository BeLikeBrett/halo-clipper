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

/// Vector icons. Each is drawn inside a square and scales with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Play,
    Pause,
    SkipStart,
    Speaker,
    SpeakerMuted,
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
    }
}

/// Square icon button: transparent at rest, soft fill on hover. `on`
/// tints the icon with `on_color` (e.g. a muted track).
pub fn icon_button(ui: &mut Ui, icon: Icon, size: f32, on: bool, on_color: Color32) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    paint_icon_button(ui, rect, &response, icon, on, on_color);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
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

/// The circular transport button: filled, the one strong shape in the bar.
pub fn play_button(ui: &mut Ui, playing: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(vec2(34.0, 34.0), Sense::click());
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

// ── Top-bar drop-down menus ─────────────────────────────────────────────

/// Which top-bar menu is open, shared so hovering a neighbour while one is
/// open slides straight across like a native menu bar.
fn open_menu_id() -> Id {
    Id::new("halo_menu_bar_open")
}

/// Cubic ease-out: fast start, gentle landing.
fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

/// A menu-bar button whose popup grows down out of the button itself: the
/// open button and the panel share one fill, and the panel unrolls from
/// the button's lower edge instead of appearing as a detached box.
///
/// `add_contents` returns `true` when an item was chosen (closes the menu).
pub fn menu_bar_button(
    ui: &mut Ui,
    label: &str,
    min_popup_width: f32,
    add_contents: impl FnOnce(&mut Ui) -> bool,
) {
    let id = ui.id().with(("menu_bar", label));
    let ctx = ui.ctx().clone();
    let font = theme::medium(12.5);
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font, theme::TEXT_PRIMARY);
    let size = vec2(galley.size().x + 22.0, 28.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let open_now = ctx.data(|d| d.get_temp::<Id>(open_menu_id())) == Some(id);

    let mut open = open_now;
    if response.clicked() {
        open = !open_now;
    } else if response.hovered() && !open_now {
        // Menu-bar convention: once any menu is open, hovering a sibling
        // switches to it.
        if ctx.data(|d| d.get_temp::<Id>(open_menu_id())).is_some() {
            open = true;
        }
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

    let t = ctx.animate_bool_with_time(id.with("reveal"), open, 0.16);
    let eased = ease_out(t);
    let joined = Rounding {
        nw: 6.0,
        ne: 6.0,
        sw: 6.0 * (1.0 - eased),
        se: 6.0 * (1.0 - eased),
    };
    let fill = if t > 0.0 {
        lerp_color(theme::BG_BUTTON_HOVER, theme::BG_MODAL, eased)
    } else if response.hovered() {
        theme::BG_BUTTON_HOVER
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, joined, fill);
    ui.painter().galley(
        rect.center() - galley.size() * 0.5,
        galley,
        if open || response.hovered() {
            theme::TEXT_PRIMARY
        } else {
            theme::TEXT_SECONDARY
        },
    );
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);

    if t <= 0.0 {
        return;
    }
    let height_id = id.with("height");
    let full_height = ctx.data(|d| d.get_temp::<f32>(height_id)).unwrap_or(120.0);
    let top = rect.left_bottom();
    let mut chosen = false;
    let area = egui::Area::new(id.with("popup"))
        .order(egui::Order::Foreground)
        .fixed_pos(top)
        .constrain(false)
        .interactable(open)
        .show(&ctx, |ui| {
            // Unroll from the button edge: the clip grows downward while
            // the content fades in, so the panel reads as one surface
            // sliding out of the button.
            let reveal = full_height * eased;
            ui.set_clip_rect(Rect::from_min_max(
                top - vec2(40.0, 0.0),
                pos2(top.x + 600.0, top.y + reveal + 40.0 * eased),
            ));
            ui.multiply_opacity(0.35 + 0.65 * eased);
            let frame = egui::Frame::none()
                .fill(theme::BG_MODAL)
                .rounding(Rounding {
                    nw: 0.0,
                    ne: 8.0,
                    sw: 8.0,
                    se: 8.0,
                })
                .shadow(ui.visuals().popup_shadow)
                .inner_margin(egui::Margin::same(5.0));
            let inner = frame.show(ui, |ui| {
                ui.set_width(min_popup_width.max(rect.width()));
                ui.spacing_mut().item_spacing.y = 1.0;
                add_contents(ui)
            });
            chosen = inner.inner;
            inner.response.rect
        });
    let popup_rect = area.inner;
    if (popup_rect.height() - full_height).abs() > 0.5 {
        ctx.data_mut(|d| d.insert_temp(height_id, popup_rect.height()));
        ctx.request_repaint();
    }
    let clicked_elsewhere = ctx.input(|i| {
        i.pointer.any_pressed()
            && i.pointer
                .interact_pos()
                .is_some_and(|p| !popup_rect.contains(p) && !response.rect.contains(p))
    });
    let escape = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    if open && (chosen || clicked_elsewhere || escape) {
        ctx.data_mut(|d| d.remove::<Id>(open_menu_id()));
        ctx.request_repaint();
    }
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
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
    let (rect, response) = ui.allocate_exact_size(vec2(width, 28.0), Sense::click());
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
pub fn segmented(
    ui: &mut Ui,
    id: &str,
    options: &[(&str, &str)],
    selected: usize,
) -> Option<usize> {
    let font = theme::medium(12.5);
    let widths: Vec<f32> = options
        .iter()
        .map(|(label, _)| {
            ui.painter()
                .layout_no_wrap((*label).to_owned(), font.clone(), theme::TEXT_PRIMARY)
                .size()
                .x
                + 28.0
        })
        .collect();
    let pad = 3.0;
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
            .interact(rect, anim_id.with(i), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(*tip);
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
