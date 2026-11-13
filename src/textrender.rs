//! Rasterizes text layers. One code path serves both the live preview
//! (rendered at screen resolution into an egui texture) and the export
//! (rendered at output resolution into PNG frames that ffmpeg overlays),
//! so the burned-in text matches the preview exactly.
//!
//! Layout uses the bundled fonts through ab_glyph; drawing uses tiny-skia
//! for anti-aliased fills, true stroked outlines and rounded boxes.

use crate::text::{self, Align, Look, Reveal, Shadow, TextItem};
use ab_glyph::{Font, FontRef, GlyphId, OutlineCurve, PxScale, ScaleFont};
use std::path::{Path, PathBuf};
use tiny_skia::{FillRule, LineJoin, Paint, PathBuilder, Pixmap, PixmapPaint, Stroke, Transform};

/// Lines wrap at this fraction of the canvas width.
const WRAP_WIDTH: f32 = 0.9;
/// Box padding around each line, in font-size units.
const BOX_PAD_X: f32 = 0.3;
const BOX_PAD_Y: f32 = 0.1;
/// Upward/downward travel of the Rise/Drop animations, font-size units
/// (matches the 0.6 in `text::look_at`).
const SLIDE: f32 = 0.6;
/// Headroom for pop overshoot.
const OVERSHOOT: f32 = 1.12;

fn font(f: text::TextFont) -> FontRef<'static> {
    // Fonts are compiled in; a parse failure is a build-time asset bug.
    FontRef::try_from_slice(f.data()).expect("bundled caption font is valid")
}

/// Font size in pixels for a canvas of `w`×`h` pixels.
pub fn font_px(item: &TextItem, w: f32, h: f32) -> f32 {
    (item.size * w.min(h)).max(1.0)
}

#[derive(Clone, Copy, Debug)]
struct Glyph {
    id: GlyphId,
    /// Pen position relative to the line's left edge.
    x: f32,
    advance: f32,
    char_index: usize,
    word: usize,
    is_space: bool,
}

#[derive(Clone, Debug, Default)]
struct Line {
    glyphs: Vec<Glyph>,
    width: f32,
    /// Alignment offset from the block's left edge.
    offset: f32,
    /// Top of the line box relative to the block's top edge.
    top: f32,
    /// Character index where the line starts (for the caret on an empty line).
    first_char: usize,
}

/// Positioned glyphs for one text block, in pixels, block origin top-left.
#[derive(Clone, Debug)]
pub struct Layout {
    lines: Vec<Line>,
    pub width: f32,
    pub height: f32,
    px: f32,
    ascent: f32,
    line_height: f32,
}

/// Lays out `text` at `px`, wrapping at `max_width`. Character and word
/// indices follow `text.chars()` / `split_whitespace()` so animation
/// reveals line up with the source string.
/// `tracking` is extra space after every glyph (outlined text needs room
/// so strokes don't fuse neighbouring letters).
fn layout_text(
    f: &FontRef<'static>,
    px: f32,
    text: &str,
    max_width: f32,
    align: Align,
    tracking: f32,
) -> Layout {
    let scaled = f.as_scaled(PxScale::from(px));
    let ascent = scaled.ascent();
    let line_height = (scaled.ascent() - scaled.descent() + scaled.line_gap()).max(px) * 1.05;

    // Word index of every character, matching `split_whitespace()`.
    let word_of: Vec<usize> = text
        .chars()
        .scan((0usize, true), |(count, gap), ch| {
            if ch.is_whitespace() {
                *gap = true;
            } else if *gap {
                *count += 1;
                *gap = false;
            }
            Some(count.saturating_sub(1))
        })
        .collect();

    let mut lines: Vec<Line> = Vec::new();
    let mut char_index = 0usize;

    for paragraph in text.split('\n') {
        // Tokens: runs of non-space characters, each with the spaces that
        // follow it, so wrapping happens between words.
        let mut line = Line {
            first_char: char_index,
            ..Line::default()
        };
        let mut pen = 0.0f32;
        let mut previous: Option<GlyphId> = None;
        let chars: Vec<char> = paragraph.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            let start = i;
            while i < chars.len() && !chars[i].is_whitespace() {
                i += 1;
            }
            while i < chars.len() && chars[i].is_whitespace() {
                i += 1;
            }
            let token = &chars[start..i];
            let token_width: f32 = {
                let mut w = 0.0;
                let mut prev = previous;
                for ch in token.iter().take_while(|c| !c.is_whitespace()) {
                    let id = f.glyph_id(*ch);
                    if let Some(p) = prev {
                        w += scaled.kern(p, id);
                    }
                    w += scaled.h_advance(id) + tracking;
                    prev = Some(id);
                }
                w
            };
            if !line.glyphs.is_empty() && pen + token_width > max_width {
                line.width = line_ink_width(&line);
                lines.push(std::mem::take(&mut line));
                line.first_char = char_index + start;
                pen = 0.0;
                previous = None;
            }
            for (k, ch) in token.iter().enumerate() {
                let index = char_index + start + k;
                // Leading spaces on a wrapped line are dropped.
                if ch.is_whitespace() && line.glyphs.is_empty() {
                    line.first_char = index + 1;
                    continue;
                }
                let id = f.glyph_id(*ch);
                if let Some(p) = previous {
                    pen += scaled.kern(p, id);
                }
                let advance = scaled.h_advance(id);
                line.glyphs.push(Glyph {
                    id,
                    x: pen,
                    advance,
                    char_index: index,
                    word: word_of.get(index).copied().unwrap_or(0),
                    is_space: ch.is_whitespace(),
                });
                pen += advance + tracking;
                previous = Some(id);
            }
        }
        line.width = line_ink_width(&line);
        lines.push(line);
        char_index += chars.len() + 1; // + the '\n'
    }

    let width = lines.iter().map(|l| l.width).fold(0.0, f32::max);
    for (n, line) in lines.iter_mut().enumerate() {
        line.top = n as f32 * line_height;
        line.offset = match align {
            Align::Left => 0.0,
            Align::Center => (width - line.width) * 0.5,
            Align::Right => width - line.width,
        };
    }
    Layout {
        height: lines.len().max(1) as f32 * line_height,
        lines,
        width,
        px,
        ascent,
        line_height,
    }
}

/// Width up to the end of the last non-space glyph.
fn line_ink_width(line: &Line) -> f32 {
    line.glyphs
        .iter()
        .rev()
        .find(|g| g.advance > 0.0 && !g.is_space)
        .map(|g| g.x + g.advance)
        .unwrap_or(0.0)
}

/// Layout of `item` for a canvas of `w`×`h` pixels.
pub fn layout(item: &TextItem, w: f32, h: f32) -> Layout {
    let px = font_px(item, w, h);
    layout_text(
        &font(item.style.font),
        px,
        item.shown_text(),
        w * WRAP_WIDTH,
        item.style.align,
        item.style.outline * px * 0.7,
    )
}

/// Extra pixels the decoration (outline, box, shadow) adds around the
/// glyph block on each side.
fn decoration_pad(item: &TextItem, px: f32) -> (f32, f32) {
    let s = &item.style;
    let outline = s.outline * px;
    let (bx, by) = if s.background.is_some() {
        (BOX_PAD_X * px, BOX_PAD_Y * px)
    } else {
        (0.0, 0.0)
    };
    let shadow = match s.shadow {
        Shadow::None => 0.0,
        Shadow::Soft => soft_shadow(px).0 * 3.0 + soft_shadow(px).1,
        Shadow::Glow => glow_radius(px) * 3.0,
    };
    (outline + bx + shadow + 2.0, outline + by + shadow + 2.0)
}

/// Size of the visible block (glyphs + box + outline), for hit-testing
/// and selection outlines. Excludes the shadow and animation headroom.
pub fn block_size(item: &TextItem, w: f32, h: f32) -> (f32, f32) {
    let lay = layout(item, w, h);
    let s = &item.style;
    let outline = s.outline * lay.px;
    let (bx, by) = if s.background.is_some() {
        (BOX_PAD_X * lay.px, BOX_PAD_Y * lay.px)
    } else {
        (0.0, 0.0)
    };
    (
        lay.width + 2.0 * (outline + bx),
        lay.height + 2.0 * (outline + by),
    )
}

/// Fixed frame size for every animation state of `item`, so exported
/// frames all share one overlay position.
fn frame_size(item: &TextItem, lay: &Layout) -> (u32, u32) {
    let (pad_x, pad_y) = decoration_pad(item, lay.px);
    let half_w = (lay.width * 0.5 + pad_x) * OVERSHOOT;
    let half_h = (lay.height * 0.5 + pad_y) * OVERSHOOT + SLIDE * lay.px;
    (
        (half_w * 2.0).ceil().max(1.0) as u32,
        (half_h * 2.0).ceil().max(1.0) as u32,
    )
}

/// (blur radius, vertical offset) of the soft drop shadow.
fn soft_shadow(px: f32) -> (f32, f32) {
    ((px * 0.07).max(1.0), (px * 0.06).max(1.0))
}

fn glow_radius(px: f32) -> f32 {
    (px * 0.16).max(1.5)
}

/// Rendered text block. `pixmap` is premultiplied RGBA, centered on the
/// layer's (x, y) position.
pub struct Rendered {
    pub pixmap: Pixmap,
}

/// Gradient coordinates share the block's layout, so color flows across
/// whole words/lines instead of restarting at each glyph.
fn color_paint(
    rgb: [u8; 3],
    gradient: Option<text::Gradient>,
    alpha: f32,
    lay: &Layout,
) -> Paint<'static> {
    let mut paint = Paint {
        anti_alias: true,
        ..Paint::default()
    };
    let color = |c: [u8; 3]| {
        tiny_skia::Color::from_rgba8(c[0], c[1], c[2], (alpha.clamp(0.0, 1.0) * 255.0) as u8)
    };
    paint.set_color(color(rgb));
    if let Some(g) = gradient {
        let (dy, dx) = g.angle.to_radians().sin_cos();
        let span = (lay.width * dx.abs() + lay.height * dy.abs()).max(1.0) * 0.5;
        let (cx, cy) = (lay.width * 0.5, lay.height * 0.5);
        if let Some(shader) = tiny_skia::LinearGradient::new(
            tiny_skia::Point::from_xy(cx - dx * span, cy - dy * span),
            tiny_skia::Point::from_xy(cx + dx * span, cy + dy * span),
            vec![
                tiny_skia::GradientStop::new(0.0, color(rgb)),
                tiny_skia::GradientStop::new(1.0, color(g.end)),
            ],
            tiny_skia::SpreadMode::Pad,
            Transform::identity(),
        ) {
            paint.shader = shader;
        }
    }
    paint
}

/// Renders `item` in animation state `look` for a `w`×`h`-pixel canvas.
/// Returns `None` when nothing would be visible.
pub fn render(item: &TextItem, look: &Look, w: f32, h: f32) -> Option<Rendered> {
    let f = font(item.style.font);
    let lay = layout(item, w, h);
    let (fw, fh) = frame_size(item, &lay);
    let mut frame = Pixmap::new(fw, fh)?;
    if !look.is_visible() {
        return Some(Rendered { pixmap: frame });
    }
    let px = lay.px;
    let style = &item.style;
    let scaled = f.as_scaled(PxScale::from(px));
    let factor = scaled.h_scale_factor();
    let v_factor = scaled.v_scale_factor();

    // Block-local → frame: center the block, then apply the animation's
    // scale (about the center) and vertical travel.
    let base = Transform::from_translate(fw as f32 * 0.5, fh as f32 * 0.5 + look.dy * px)
        .pre_scale(look.scale, look.scale)
        .pre_translate(-lay.width * 0.5, -lay.height * 0.5);

    // Per-glyph visibility for the reveal animations.
    struct Shape {
        path: tiny_skia::Path,
        transform: Transform,
        alpha: f32,
    }
    let mut shapes: Vec<Shape> = Vec::new();
    // Visible horizontal extent per line, for boxes.
    let mut extents: Vec<Option<(f32, f32)>> = vec![None; lay.lines.len()];
    let mut caret: Option<(usize, f32)> = None;
    let shown_chars = match &look.reveal {
        Reveal::Chars { shown, caret: on } => {
            if *on {
                caret = Some((0, lay.lines.first().map(|l| l.offset).unwrap_or(0.0)));
            }
            Some(*shown)
        }
        _ => None,
    };

    // Word centers, for scaling words about their own middle.
    let mut word_bounds: Vec<(f32, f32, f32)> = Vec::new(); // (x0, x1, baseline_mid)
    for line in &lay.lines {
        for g in &line.glyphs {
            if g.word >= word_bounds.len() {
                word_bounds.resize(g.word + 1, (f32::MAX, f32::MIN, 0.0));
            }
            let b = &mut word_bounds[g.word];
            b.0 = b.0.min(line.offset + g.x);
            b.1 = b.1.max(line.offset + g.x + g.advance);
            b.2 = line.top + lay.line_height * 0.5;
        }
    }

    for (n, line) in lay.lines.iter().enumerate() {
        let baseline =
            line.top + lay.ascent + (lay.line_height - (scaled.ascent() - scaled.descent())) * 0.5;
        if let (Some(shown), Some(_)) = (shown_chars, caret) {
            if line.first_char <= shown {
                caret = Some((n, line.offset));
            }
        }
        for g in &line.glyphs {
            let (alpha, scale) = match &look.reveal {
                Reveal::All => (1.0, 1.0),
                Reveal::Chars { shown, .. } => {
                    if g.char_index < *shown {
                        (1.0, 1.0)
                    } else {
                        (0.0, 1.0)
                    }
                }
                Reveal::Words(progress) => {
                    text::word_pop(progress.get(g.word).copied().unwrap_or(1.0))
                }
            };
            if alpha <= 0.0 || scale <= 0.0 {
                continue;
            }
            let x0 = line.offset + g.x;
            if shown_chars.is_some() {
                caret = Some((n, x0 + g.advance));
            }
            let ext = extents[n].get_or_insert((x0, x0 + g.advance));
            ext.0 = ext.0.min(x0);
            ext.1 = ext.1.max(x0 + g.advance);

            let Some(outline) = f.outline(g.id) else {
                continue; // spaces
            };
            let Some(path) = glyph_path(&outline.curves, x0, baseline, factor, v_factor) else {
                continue;
            };
            let transform = if (scale - 1.0).abs() > f32::EPSILON {
                let (wx0, wx1, wy) = word_bounds[g.word];
                let cx = (wx0 + wx1) * 0.5;
                base.pre_translate(cx, wy)
                    .pre_scale(scale, scale)
                    .pre_translate(-cx, -wy)
            } else {
                base
            };
            shapes.push(Shape {
                path,
                transform,
                alpha,
            });
        }
    }

    // Typewriter caret.
    if let Some((n, x)) = caret.filter(|_| matches!(look.reveal, Reveal::Chars { caret: true, .. }))
    {
        let line = &lay.lines[n.min(lay.lines.len() - 1)];
        let height = (scaled.ascent() - scaled.descent()) * 0.82;
        let top = line.top + (lay.line_height - height) * 0.5;
        let width = (px * 0.07).max(1.0);
        if let Some(rect) = tiny_skia::Rect::from_xywh(x + px * 0.04, top, width, height) {
            shapes.push(Shape {
                path: PathBuilder::from_rect(rect),
                transform: base,
                alpha: 1.0,
            });
            let ext = extents[n].get_or_insert((x, x + width));
            ext.1 = ext.1.max(x + px * 0.04 + width);
        }
    }

    // Background boxes, one per line with visible text.
    let boxes = style.background.and_then(|(rgb, _)| {
        let mut pb = PathBuilder::new();
        let mut any = false;
        for (line, ext) in lay.lines.iter().zip(&extents) {
            let Some((x0, x1)) = ext else { continue };
            let rect = tiny_skia::Rect::from_ltrb(
                x0 - BOX_PAD_X * px,
                line.top - BOX_PAD_Y * px + (lay.line_height - px * 1.2).max(0.0) * 0.5,
                x1 + BOX_PAD_X * px,
                line.top + lay.line_height + BOX_PAD_Y * px
                    - (lay.line_height - px * 1.2).max(0.0) * 0.5,
            )?;
            push_round_rect(&mut pb, rect, (px * 0.24).min(rect.height() * 0.5));
            any = true;
        }
        any.then(|| pb.finish()).flatten().map(|p| (p, rgb))
    });

    let paint = Paint {
        anti_alias: true,
        ..Paint::default()
    };
    let outline_w = style.outline * px;
    let stroke = Stroke {
        width: outline_w * 2.0,
        line_join: LineJoin::Round,
        ..Stroke::default()
    };

    // Silhouette (box, or outline+fill) in one color, for shadow / glow.
    let silhouette = |rgb: [u8; 3]| -> Option<Pixmap> {
        let mut layer = Pixmap::new(fw, fh)?;
        let mut p = paint.clone();
        p.set_color_rgba8(rgb[0], rgb[1], rgb[2], 255);
        if let Some((path, _)) = &boxes {
            layer.fill_path(path, &p, FillRule::Winding, base, None);
        } else {
            for s in &shapes {
                p.set_color_rgba8(rgb[0], rgb[1], rgb[2], (s.alpha * 255.0) as u8);
                if outline_w > 0.0 {
                    layer.stroke_path(&s.path, &p, &stroke, s.transform, None);
                }
                layer.fill_path(&s.path, &p, FillRule::Winding, s.transform, None);
            }
        }
        Some(layer)
    };
    let scale_px = px * look.scale;
    match style.shadow {
        Shadow::None => {}
        Shadow::Soft => {
            if let Some(mut layer) = silhouette([0, 0, 0]) {
                let (radius, offset) = soft_shadow(scale_px);
                blur(&mut layer, radius);
                frame.draw_pixmap(
                    0,
                    offset.round() as i32,
                    layer.as_ref(),
                    &PixmapPaint {
                        opacity: 0.6,
                        ..PixmapPaint::default()
                    },
                    Transform::identity(),
                    None,
                );
            }
        }
        Shadow::Glow => {
            if let Some(layer) = silhouette(style.color) {
                for (radius, opacity) in [
                    (glow_radius(scale_px), 0.9),
                    (glow_radius(scale_px) * 0.35, 0.8),
                ] {
                    let mut glow = layer.clone();
                    blur(&mut glow, radius);
                    frame.draw_pixmap(
                        0,
                        0,
                        glow.as_ref(),
                        &PixmapPaint {
                            opacity,
                            ..PixmapPaint::default()
                        },
                        Transform::identity(),
                        None,
                    );
                }
            }
        }
    }

    if let Some((path, rgb)) = &boxes {
        let opacity = style.background.map(|(_, a)| a).unwrap_or(1.0);
        if let Some(mut layer) = Pixmap::new(fw, fh) {
            let p = color_paint(*rgb, style.box_gradient, 1.0, &lay);
            layer.fill_path(path, &p, FillRule::Winding, base, None);
            frame.draw_pixmap(
                0,
                0,
                layer.as_ref(),
                &PixmapPaint {
                    opacity: opacity.clamp(0.0, 1.0),
                    ..PixmapPaint::default()
                },
                Transform::identity(),
                None,
            );
        }
    }

    // Outlines first, then fills, so a stroke never covers its
    // neighbour's face.
    if outline_w > 0.0 {
        for s in &shapes {
            let paint = color_paint(style.outline_color, style.outline_gradient, s.alpha, &lay);
            frame.stroke_path(&s.path, &paint, &stroke, s.transform, None);
        }
    }
    for s in &shapes {
        let paint = color_paint(style.color, style.gradient, s.alpha, &lay);
        frame.fill_path(&s.path, &paint, FillRule::Winding, s.transform, None);
    }

    if look.alpha < 1.0 {
        let a = look.alpha.clamp(0.0, 1.0);
        for v in frame.data_mut() {
            *v = (*v as f32 * a).round() as u8;
        }
    }
    Some(Rendered { pixmap: frame })
}

/// Builds a glyph's path from its font-unit outline (y up) at pen
/// position `x`, `baseline`.
fn glyph_path(
    curves: &[OutlineCurve],
    x: f32,
    baseline: f32,
    sx: f32,
    sy: f32,
) -> Option<tiny_skia::Path> {
    let map = |p: ab_glyph::Point| (x + p.x * sx, baseline - p.y * sy);
    let mut pb = PathBuilder::new();
    let mut last: Option<ab_glyph::Point> = None;
    for curve in curves {
        let start = match curve {
            OutlineCurve::Line(a, _)
            | OutlineCurve::Quad(a, _, _)
            | OutlineCurve::Cubic(a, _, _, _) => *a,
        };
        if last != Some(start) {
            if last.is_some() {
                pb.close();
            }
            let (mx, my) = map(start);
            pb.move_to(mx, my);
        }
        match curve {
            OutlineCurve::Line(_, b) => {
                let (bx, by) = map(*b);
                pb.line_to(bx, by);
                last = Some(*b);
            }
            OutlineCurve::Quad(_, c, b) => {
                let (cx, cy) = map(*c);
                let (bx, by) = map(*b);
                pb.quad_to(cx, cy, bx, by);
                last = Some(*b);
            }
            OutlineCurve::Cubic(_, c1, c2, b) => {
                let (ax, ay) = map(*c1);
                let (cx, cy) = map(*c2);
                let (bx, by) = map(*b);
                pb.cubic_to(ax, ay, cx, cy, bx, by);
                last = Some(*b);
            }
        }
    }
    if last.is_some() {
        pb.close();
    }
    pb.finish()
}

fn push_round_rect(pb: &mut PathBuilder, r: tiny_skia::Rect, radius: f32) {
    let k = radius.max(0.0);
    let (l, t, rt, b) = (r.left(), r.top(), r.right(), r.bottom());
    pb.move_to(l + k, t);
    pb.line_to(rt - k, t);
    pb.quad_to(rt, t, rt, t + k);
    pb.line_to(rt, b - k);
    pb.quad_to(rt, b, rt - k, b);
    pb.line_to(l + k, b);
    pb.quad_to(l, b, l, b - k);
    pb.line_to(l, t + k);
    pb.quad_to(l, t, l + k, t);
    pb.close();
}

/// Approximate Gaussian blur: three box-blur passes per axis on the
/// premultiplied pixels.
fn blur(pixmap: &mut Pixmap, radius: f32) {
    let r = radius.round().max(1.0) as usize;
    let (w, h) = (pixmap.width() as usize, pixmap.height() as usize);
    let data = pixmap.data_mut();
    let mut scratch = vec![0u8; data.len()];
    for _ in 0..3 {
        box_pass(data, &mut scratch, w, h, r, true);
        box_pass(&scratch, data, w, h, r, false);
    }
}

/// One running-sum box blur along rows (`horizontal`) or columns, edges
/// treated as transparent.
fn box_pass(src: &[u8], dst: &mut [u8], w: usize, h: usize, r: usize, horizontal: bool) {
    let (outer, inner) = if horizontal { (h, w) } else { (w, h) };
    let index = |o: usize, i: usize| {
        if horizontal {
            (o * w + i) * 4
        } else {
            (i * w + o) * 4
        }
    };
    let norm = 1.0 / (2 * r + 1) as f32;
    for o in 0..outer {
        let mut sum = [0u32; 4];
        for i in 0..r.min(inner) {
            let p = index(o, i);
            for c in 0..4 {
                sum[c] += src[p + c] as u32;
            }
        }
        for i in 0..inner {
            if i + r < inner {
                let p = index(o, i + r);
                for c in 0..4 {
                    sum[c] += src[p + c] as u32;
                }
            }
            let d = index(o, i);
            for c in 0..4 {
                dst[d + c] = (sum[c] as f32 * norm).round().min(255.0) as u8;
            }
            if i >= r {
                let p = index(o, i - r);
                for c in 0..4 {
                    sum[c] -= src[p + c] as u32;
                }
            }
        }
    }
}

// ── Export ──────────────────────────────────────────────────────────────

/// One text layer prepared for ffmpeg: a concat list of PNG frames and
/// where the frame's top-left lands on the output canvas.
pub struct ExportLayer {
    pub list: PathBuf,
    pub x: i32,
    pub y: i32,
    pub start_ms: i64,
    pub end_ms: i64,
}

/// Renders every non-empty text layer to PNG frames under `dir`. Frames
/// are only written when the animation state changes; held states become
/// long `duration` entries in the concat list.
pub fn write_export_layers(
    items: &[TextItem],
    width: u32,
    height: u32,
    fps: f64,
    dir: &Path,
    should_cancel: impl Fn() -> bool,
) -> anyhow::Result<Vec<ExportLayer>> {
    let (w, h) = (width as f32, height as f32);
    let frame_ms = 1000.0 / fps.clamp(1.0, 240.0);
    let mut layers = Vec::new();
    for (n, item) in items
        .iter()
        .filter(|t| !t.text.trim().is_empty() && t.end_ms > t.start_ms)
        .enumerate()
    {
        let frames = ((item.duration_ms() as f64) / frame_ms).ceil().max(1.0) as usize;
        let mut entries: Vec<(PathBuf, usize)> = Vec::new();
        let mut previous: Option<Look> = None;
        let mut size = (1, 1);
        for i in 0..frames {
            anyhow::ensure!(!should_cancel(), "Export cancelled");
            let t = item.start_ms + (i as f64 * frame_ms).round() as i64;
            let look = text::look_at(item, t.min(item.end_ms - 1));
            if previous.as_ref() == Some(&look) {
                if let Some(last) = entries.last_mut() {
                    last.1 += 1;
                }
                continue;
            }
            let rendered = render(item, &look, w, h)
                .ok_or_else(|| anyhow::anyhow!("text frame too large to render"))?;
            size = (rendered.pixmap.width(), rendered.pixmap.height());
            let path = dir.join(format!("text{n}_{:05}.png", entries.len()));
            std::fs::write(&path, rendered.pixmap.encode_png()?)?;
            entries.push((path, 1));
            previous = Some(look);
        }
        let mut list = String::from("ffconcat version 1.0\n");
        for (path, count) in &entries {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            list.push_str(&format!(
                "file '{name}'\noption framerate {fps:.6}\nduration {:.9}\n",
                *count as f64 * frame_ms / 1000.0
            ));
        }
        // The concat demuxer ignores the final entry's duration unless the
        // file is listed once more.
        if let Some((path, _)) = entries.last() {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            list.push_str(&format!("file '{name}'\noption framerate {fps:.6}\n"));
        }
        let list_path = dir.join(format!("text{n}.ffconcat"));
        std::fs::write(&list_path, list)?;
        layers.push(ExportLayer {
            list: list_path,
            x: (item.x * w - size.0 as f32 * 0.5).round() as i32,
            y: (item.y * h - size.1 as f32 * 0.5).round() as i32,
            start_ms: item.start_ms,
            end_ms: item.end_ms,
        });
    }
    Ok(layers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::{Intro, TextFont, TextId, STYLE_PRESETS};

    fn item(text: &str) -> TextItem {
        let mut t = TextItem::new(TextId(1), 0, 2_000);
        t.text = text.into();
        t
    }

    fn ink(p: &Pixmap) -> usize {
        p.data().chunks(4).filter(|px| px[3] > 0).count()
    }

    #[test]
    fn gradients_color_text_outline_and_box_and_invalidate_cache() {
        for target in 0..3 {
            let mut t = item("MMMMMM");
            t.style.shadow = Shadow::None;
            t.style.outline = 0.0;
            t.style.background = None;
            t.style.color = [255, 255, 255];
            let gradient = Some(text::Gradient {
                end: [0, 0, 255],
                angle: 0.0,
            });
            let before = t.render_key();
            match target {
                0 => {
                    t.style.color = [255, 0, 0];
                    t.style.gradient = gradient;
                }
                1 => {
                    t.style.outline = 0.15;
                    t.style.outline_color = [255, 0, 0];
                    t.style.outline_gradient = gradient;
                }
                _ => {
                    t.style.background = Some(([255, 0, 0], 1.0));
                    t.style.box_gradient = gradient;
                }
            }
            assert_ne!(t.render_key(), before);
            let pix = render(&t, &Look::STILL, 1080.0, 1920.0).unwrap().pixmap;
            assert!(
                pix.pixels().iter().any(|p| p.alpha() > 240
                    && p.red().saturating_sub(p.blue()) > 30
                    && p.green() < 80),
                "target {target}"
            );
            assert!(
                pix.pixels().iter().any(|p| p.alpha() > 240
                    && p.blue().saturating_sub(p.red()) > 30
                    && p.green() < 80),
                "target {target}"
            );
        }
    }

    #[test]
    fn every_font_renders_ink() {
        for f in TextFont::ALL {
            let mut t = item("Hello");
            t.style.font = f;
            let r = render(&t, &Look::STILL, 1080.0, 1920.0).unwrap();
            assert!(ink(&r.pixmap) > 200, "{f:?}");
        }
    }

    #[test]
    fn every_preset_renders() {
        for (name, preset) in STYLE_PRESETS {
            let mut t = item("Hello there");
            t.style = preset;
            let r = render(&t, &Look::STILL, 1920.0, 1080.0).unwrap();
            assert!(ink(&r.pixmap) > 200, "{name}");
        }
    }

    #[test]
    fn long_text_wraps_inside_the_canvas() {
        let t = item("this is a fairly long caption that cannot possibly fit on one line");
        let lay = layout(&t, 1080.0, 1920.0);
        assert!(lay.lines.len() > 1);
        assert!(lay.width <= 1080.0 * WRAP_WIDTH + 1.0);
    }

    #[test]
    fn words_and_chars_index_the_source_text() {
        let t = item("one two\nthree");
        let lay = layout(&t, 1920.0, 1080.0);
        let words: Vec<usize> = lay
            .lines
            .iter()
            .flat_map(|l| l.glyphs.iter())
            .filter(|g| !g.is_space)
            .map(|g| g.word)
            .collect();
        assert_eq!(words.first(), Some(&0));
        assert_eq!(words.last(), Some(&2));
        let last = lay.lines[1].glyphs.last().unwrap();
        assert_eq!(last.char_index, "one two\nthree".chars().count() - 1);
    }

    #[test]
    fn typewriter_frames_grow_with_the_reveal() {
        let mut t = item("Typing");
        t.intro = Intro::Typewriter;
        let at = |shown| {
            let look = Look {
                reveal: Reveal::Chars {
                    shown,
                    caret: false,
                },
                ..Look::STILL
            };
            ink(&render(&t, &look, 1920.0, 1080.0).unwrap().pixmap)
        };
        assert_eq!(at(0), 0);
        assert!(at(2) < at(4));
        assert!(at(4) < at(6));
    }

    #[test]
    fn frame_size_is_constant_across_the_animation() {
        let mut t = item("Pop!");
        t.intro = Intro::Pop;
        let sizes: std::collections::HashSet<_> = (0..2_000)
            .step_by(40)
            .map(|ms| {
                let r = render(&t, &text::look_at(&t, ms), 1920.0, 1080.0).unwrap();
                (r.pixmap.width(), r.pixmap.height())
            })
            .collect();
        assert_eq!(sizes.len(), 1);
    }

    #[test]
    fn export_preparation_can_be_cancelled() {
        let dir = std::env::temp_dir().join(format!("halo-text-cancel-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let result = write_export_layers(&[item("Cancel")], 1280, 720, 60.0, &dir, || true);
        assert!(result.is_err());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn export_animation_keeps_sixty_fps_timestamps() {
        let dir = std::env::temp_dir().join(format!("halo-text-fps-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let layers =
            write_export_layers(&[item("Smooth")], 320, 180, 60.0, &dir, || false).unwrap();
        let probe = crate::platform::command("ffprobe")
            .args(["-v", "error", "-f", "concat", "-safe", "0", "-i"])
            .arg(&layers[0].list)
            .args(["-show_entries", "packet=pts_time", "-of", "json"])
            .output()
            .unwrap();
        assert!(
            probe.status.success(),
            "{}",
            String::from_utf8_lossy(&probe.stderr)
        );
        let packets: serde_json::Value = serde_json::from_slice(&probe.stdout).unwrap();
        let times: Vec<f64> = packets["packets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["pts_time"].as_str().unwrap().parse().unwrap())
            .collect();
        for (index, time) in times.iter().take(8).enumerate() {
            assert!(
                (time - index as f64 / 60.0).abs() < 0.00001,
                "frame {index}: {time}"
            );
        }
        assert!((times.last().unwrap() - 2.0).abs() < 0.00001);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn export_reuses_frames_for_held_states() {
        let dir = std::env::temp_dir().join(format!("halo-text-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut t = item("Hold");
        t.end_ms = 3_000;
        let layers = write_export_layers(&[t], 1280, 720, 30.0, &dir, || false).unwrap();
        assert_eq!(layers.len(), 1);
        let pngs = std::fs::read_dir(&dir)
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .is_ok_and(|e| e.path().extension().is_some_and(|x| x == "png"))
            })
            .count();
        // Pop in + fade out frames, not 90 copies of the held state.
        assert!(pngs < 40, "{pngs}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// Writes sample renders of every preset, font and animation to
/// `$HALO_TEXT_SAMPLES` for eyeballing: `HALO_TEXT_SAMPLES=/tmp/x cargo
/// test --release dump_text_samples -- --ignored`.
#[cfg(test)]
#[test]
#[ignore]
fn dump_text_samples() {
    use crate::text::{Intro, TextFont, TextId, STYLE_PRESETS};
    let Some(dir) = std::env::var_os("HALO_TEXT_SAMPLES") else {
        return;
    };
    let dir = PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    let base = |text: &str| {
        let mut t = TextItem::new(TextId(1), 0, 3_000);
        t.text = text.into();
        t
    };
    for (name, preset) in STYLE_PRESETS {
        let mut t = base("When the clutch\nhits different");
        t.style = preset;
        let r = render(&t, &Look::STILL, 1080.0, 1920.0).unwrap();
        std::fs::write(
            dir.join(format!("preset-{name}.png")),
            r.pixmap.encode_png().unwrap(),
        )
        .unwrap();
    }
    for f in TextFont::ALL {
        let mut t = base(f.label());
        t.style.font = f;
        let r = render(&t, &Look::STILL, 1080.0, 1920.0).unwrap();
        std::fs::write(
            dir.join(format!("font-{}.png", f.label())),
            r.pixmap.encode_png().unwrap(),
        )
        .unwrap();
    }
    for intro in [Intro::Pop, Intro::Typewriter, Intro::Words] {
        let mut t = base("Wait for it, here it comes");
        t.intro = intro;
        for ms in [60, 150, 400, 900] {
            let r = render(&t, &text::look_at(&t, ms), 1080.0, 1920.0).unwrap();
            std::fs::write(
                dir.join(format!("anim-{intro:?}-{ms:04}.png")),
                r.pixmap.encode_png().unwrap(),
            )
            .unwrap();
        }
    }
}
