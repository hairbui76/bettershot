//! Font loading, text layout and glyph rasterization.
//!
//! # Why a runtime font search instead of an embedded face
//!
//! Embedding a font would add ~700 KB to every build and would still be the
//! wrong face on most desktops. Instead we look for a well-known sans-serif in
//! the usual places on each platform, cache the first one that parses, and
//! remember which file it came from so bug reports can say.
//!
//! # Why a missing font is not a fatal error
//!
//! An export that dies because `/usr/share/fonts` is empty is worse than an
//! export with ugly text: the pixels the user cares about — the arrows, the
//! blur over their password — are all still correct. So [`Font::system`] falls
//! back to drawing a filled block per visible character, using exactly the
//! metrics of [`bettershot_core::painter::estimate_text_size`] so that layout
//! stays self-consistent. Callers that would rather refuse can use
//! [`Font::try_system`], which reports [`crate::RenderError::FontNotFound`].

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use ab_glyph::{Font as AbFont, FontVec, GlyphId, PxScale, ScaleFont as AbScaleFont};
use bettershot_core::math::{Rect, Vec2D};
use bettershot_core::painter::{TextAlign, TextDraw};
use bettershot_core::style::Color;

use crate::canvas::Canvas;
use crate::error::RenderError;
use crate::raster::fill_axis_rect;

/// Line advance as a multiple of the em size.
pub const LINE_HEIGHT_FACTOR: f32 = 1.2;
/// Caret thickness as a multiple of the em size, with a one-pixel floor.
const CARET_WIDTH_FACTOR: f32 = 0.06;
/// Padding around the text block when a background plate is requested.
const BACKGROUND_PAD_FACTOR: f32 = 0.15;
/// Glyphs placed further than this from the origin are rejected rather than
/// handed to the rasterizer, which would otherwise try to allocate a bitmap.
const MAX_GLYPH_COORD: f32 = 1.0e6;

/// Environment variable that overrides the font search entirely. Handy for
/// reproducible screenshots in CI.
pub const FONT_ENV_VAR: &str = "BETTERSHOT_FONT";

/// Candidate font files, most preferred first. Missing paths are skipped, so
/// the list can carry all three platforms unconditionally.
pub const FONT_SEARCH_PATHS: &[&str] = &[
    // Linux
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
    "/usr/share/fonts/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
    "/usr/share/fonts/liberation-sans/LiberationSans-Regular.ttf",
    "/usr/share/fonts/truetype/freefont/FreeSans.ttf",
    "/usr/share/fonts/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/cantarell/Cantarell-Regular.otf",
    // Windows
    r"C:\Windows\Fonts\segoeui.ttf",
    r"C:\Windows\Fonts\arial.ttf",
    r"C:\Windows\Fonts\tahoma.ttf",
    // macOS
    "/System/Library/Fonts/SFNS.ttf",
    "/System/Library/Fonts/SFNSText.ttf",
    "/System/Library/Fonts/Helvetica.ttc",
    "/Library/Fonts/Arial.ttf",
];

/// Where the loaded face came from, for logging and diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FontSource {
    File(PathBuf),
    Memory,
    /// No face; text is drawn as filled blocks.
    Fallback,
}

/// A text face, or the block-glyph fallback.
pub struct Font {
    face: Option<FontVec>,
    source: FontSource,
}

impl std::fmt::Debug for Font {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Font")
            .field("source", &self.source)
            .finish()
    }
}

impl Font {
    /// The block-glyph fallback: no real face, rectangles per character.
    pub fn fallback() -> Self {
        Self {
            face: None,
            source: FontSource::Fallback,
        }
    }

    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, RenderError> {
        let path = path.as_ref();
        let bytes = std::fs::read(path).map_err(|e| RenderError::io(path, e))?;
        let face = FontVec::try_from_vec(bytes).map_err(|_| RenderError::FontInvalid {
            path: path.to_path_buf(),
        })?;
        Ok(Self {
            face: Some(face),
            source: FontSource::File(path.to_path_buf()),
        })
    }

    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, RenderError> {
        let face = FontVec::try_from_vec(bytes).map_err(|_| RenderError::FontInvalid {
            path: PathBuf::from("<memory>"),
        })?;
        Ok(Self {
            face: Some(face),
            source: FontSource::Memory,
        })
    }

    /// Search `$BETTERSHOT_FONT` and then [`FONT_SEARCH_PATHS`].
    pub fn try_system() -> Result<Self, RenderError> {
        if let Some(path) = std::env::var_os(FONT_ENV_VAR) {
            let path = PathBuf::from(path);
            match Self::from_path(&path) {
                Ok(font) => return Ok(font),
                Err(e) => log::warn!("{FONT_ENV_VAR} points at an unusable font: {e}"),
            }
        }
        for candidate in FONT_SEARCH_PATHS {
            let path = Path::new(candidate);
            if !path.is_file() {
                continue;
            }
            match Self::from_path(path) {
                Ok(font) => {
                    log::debug!("using font {candidate}");
                    return Ok(font);
                }
                Err(e) => log::debug!("skipping {candidate}: {e}"),
            }
        }
        Err(RenderError::FontNotFound {
            searched: FONT_SEARCH_PATHS.len(),
        })
    }

    /// Like [`Font::try_system`] but degrades to [`Font::fallback`] rather than
    /// failing, so a render never dies over a missing font.
    pub fn system() -> Self {
        Self::try_system().unwrap_or_else(|e| {
            log::warn!("{e}; falling back to block glyphs");
            Font::fallback()
        })
    }

    pub fn source(&self) -> &FontSource {
        &self.source
    }

    pub fn is_fallback(&self) -> bool {
        self.face.is_none()
    }

    /// Size of the text block: widest line by total line advance.
    pub fn measure(&self, text: &str, size: f32) -> Vec2D {
        self.layout(text, size).block
    }

    fn layout(&self, text: &str, size: f32) -> Layout {
        let size = if size.is_finite() && size > 0.0 {
            size
        } else {
            0.0
        };
        let line_height = size * LINE_HEIGHT_FACTOR;
        let mut lines = Vec::new();
        let mut byte_start = 0usize;

        for raw in text.split('\n') {
            lines.push(self.layout_line(raw, byte_start, size));
            // +1 for the '\n' that `split` consumed.
            byte_start += raw.len() + 1;
        }

        let widest = lines.iter().map(|l| l.width).fold(0.0f32, f32::max);
        let ascent = match &self.face {
            Some(face) => face.as_scaled(PxScale::from(size)).ascent(),
            // Keeps the fallback blocks sitting on the same baseline the real
            // metrics would produce.
            None => size * 0.8,
        };

        Layout {
            block: Vec2D::new(widest, lines.len() as f32 * line_height),
            line_height,
            ascent,
            size,
            lines,
        }
    }

    fn layout_line(&self, text: &str, byte_start: usize, size: f32) -> LineLayout {
        let mut glyphs = Vec::new();
        // A stop per character boundary plus one at the end of the line, so a
        // caret can sit anywhere including after the last glyph.
        let mut stops = Vec::new();
        let mut x = 0.0f32;

        match &self.face {
            Some(face) => {
                let scaled = face.as_scaled(PxScale::from(size));
                let mut previous: Option<GlyphId> = None;
                for (offset, ch) in text.char_indices() {
                    // A control character occupies no space and draws nothing.
                    // It still gets a caret stop so the caret can be placed
                    // either side of it, but giving it an advance would push
                    // the rest of the line along for an invisible character —
                    // which is what a pasted `\r` used to do.
                    if ch.is_control() {
                        stops.push((byte_start + offset, x));
                        continue;
                    }
                    let id = scaled.glyph_id(ch);
                    if let Some(p) = previous {
                        x += scaled.kern(p, id);
                    }
                    stops.push((byte_start + offset, x));
                    glyphs.push(PlacedGlyph { id, x, ch });
                    x += scaled.h_advance(id);
                    previous = Some(id);
                }
            }
            None => {
                // Mirrors `estimate_text_size`: a flat 0.5em advance.
                let advance = size * 0.5;
                for (offset, ch) in text.char_indices() {
                    stops.push((byte_start + offset, x));
                    if ch.is_control() {
                        continue;
                    }
                    glyphs.push(PlacedGlyph {
                        id: GlyphId(0),
                        x,
                        ch,
                    });
                    x += advance;
                }
            }
        }

        stops.push((byte_start + text.len(), x));
        LineLayout {
            glyphs,
            stops,
            width: x,
            byte_start,
            byte_end: byte_start + text.len(),
        }
    }
}

struct PlacedGlyph {
    id: GlyphId,
    /// Pen position relative to the start of the line.
    x: f32,
    ch: char,
}

struct LineLayout {
    glyphs: Vec<PlacedGlyph>,
    /// `(byte offset in the whole string, x)` for every caret position.
    stops: Vec<(usize, f32)>,
    width: f32,
    byte_start: usize,
    byte_end: usize,
}

struct Layout {
    lines: Vec<LineLayout>,
    block: Vec2D,
    line_height: f32,
    ascent: f32,
    size: f32,
}

impl Layout {
    /// Top-left corner of the block for the requested anchor.
    fn origin(&self, pos: Vec2D, align: TextAlign) -> Vec2D {
        match align {
            TextAlign::Left => pos,
            TextAlign::Center => pos - self.block * 0.5,
        }
    }

    /// Left edge of a line inside the block. Centered text centres each line,
    /// not just the block, which is what makes a two-digit marker label look
    /// right over a one-digit one.
    fn line_left(&self, line: &LineLayout, origin_x: f32, align: TextAlign) -> f32 {
        match align {
            TextAlign::Left => origin_x,
            TextAlign::Center => origin_x + (self.block.x - line.width) / 2.0,
        }
    }
}

/// Draw a text run. Never panics: bad sizes, non-finite positions and byte
/// offsets that do not land on a character boundary all degrade gracefully.
pub(crate) fn draw_text(canvas: &mut Canvas, font: &Font, draw: &TextDraw<'_>) {
    if canvas.is_empty() || !draw.pos.x.is_finite() || !draw.pos.y.is_finite() {
        return;
    }
    if !draw.size.is_finite() || draw.size <= 0.0 {
        return;
    }

    let layout = font.layout(draw.text, draw.size);
    let origin = layout.origin(draw.pos, draw.align);
    if !origin.x.is_finite() || !origin.y.is_finite() {
        return;
    }

    if let Some(background) = draw.background {
        let pad = layout.size * BACKGROUND_PAD_FACTOR;
        fill_axis_rect(
            canvas,
            Rect::new(origin, layout.block).expanded(pad),
            background,
        );
    }

    for (index, line) in layout.lines.iter().enumerate() {
        let left = layout.line_left(line, origin.x, draw.align);
        let top = origin.y + index as f32 * layout.line_height;
        draw_line(canvas, font, line, left, top, &layout, draw.color);
    }

    if let Some(cursor) = draw.cursor {
        draw_caret(canvas, &layout, origin, draw, cursor);
    }
}

fn draw_line(
    canvas: &mut Canvas,
    font: &Font,
    line: &LineLayout,
    left: f32,
    top: f32,
    layout: &Layout,
    color: Color,
) {
    let baseline = top + layout.ascent;
    match &font.face {
        Some(face) => {
            let scale = PxScale::from(layout.size);
            for placed in &line.glyphs {
                // Control characters map to glyph 0, which *has* an outline in
                // most fonts — it is the "missing glyph" box. Drawing it turns
                // a pasted tab or a Windows CRLF into a row of tofu, so skip
                // whitespace here as the fallback path already does.
                if placed.ch.is_whitespace() || placed.ch.is_control() {
                    continue;
                }
                let (x, y) = (left + placed.x, baseline);
                if x.abs() > MAX_GLYPH_COORD || y.abs() > MAX_GLYPH_COORD {
                    continue;
                }
                let glyph = placed
                    .id
                    .with_scale_and_position(scale, ab_glyph::point(x, y));
                let Some(outlined) = face.outline_glyph(glyph) else {
                    // Whitespace and unmapped code points have no outline.
                    continue;
                };
                let bounds = outlined.px_bounds();
                let (ox, oy) = (bounds.min.x.floor() as i32, bounds.min.y.floor() as i32);
                outlined.draw(|gx, gy, coverage| {
                    canvas.blend(ox + gx as i32, oy + gy as i32, color, coverage);
                });
            }
        }
        None => {
            // Block fallback: a solid slab where each visible character would
            // be, keeping the same advance as the real metrics estimate.
            let w = layout.size * 0.4;
            let h = layout.size * 0.7;
            for placed in &line.glyphs {
                if placed.ch.is_whitespace() || placed.ch.is_control() {
                    continue;
                }
                fill_axis_rect(
                    canvas,
                    Rect::from_xywh(left + placed.x, baseline - h, w, h),
                    color,
                );
            }
        }
    }
}

/// Vertical bar at `cursor`, a **byte** offset into `draw.text`.
///
/// Offsets that fall inside a multi-byte character (or past the end) are
/// clamped to the nearest caret stop rather than rejected: an IME preedit can
/// legitimately report a mid-character offset and losing the caret is worse
/// than putting it one grapheme off.
fn draw_caret(
    canvas: &mut Canvas,
    layout: &Layout,
    origin: Vec2D,
    draw: &TextDraw<'_>,
    cursor: usize,
) {
    let cursor = cursor.min(draw.text.len());
    let found = layout
        .lines
        .iter()
        .enumerate()
        .find(|(_, l)| cursor >= l.byte_start && cursor <= l.byte_end);
    // `split('\n')` always yields at least one line and the lines tile
    // `0..=text.len()`, so this only misses if the caller lied about `text`.
    let Some((index, line)) = found else { return };

    let left = layout.line_left(line, origin.x, draw.align);
    // Nearest stop at or before the offset; `stops` is sorted by construction.
    let x = line
        .stops
        .iter()
        .take_while(|(offset, _)| *offset <= cursor)
        .last()
        .map(|(_, x)| *x)
        .unwrap_or(0.0);

    let width = (layout.size * CARET_WIDTH_FACTOR).max(1.0);
    let top = origin.y + index as f32 * layout.line_height;
    fill_axis_rect(
        canvas,
        Rect::from_xywh(left + x, top, width, layout.line_height),
        draw.color,
    );
}

/// Process-wide font, loaded once. Sharing it matters: parsing a font on every
/// export would dominate the cost of rendering a small scene.
pub fn system_font() -> &'static Font {
    static FONT: OnceLock<Font> = OnceLock::new();
    FONT.get_or_init(Font::system)
}

#[cfg(test)]
mod tests {
    /// Control characters map to glyph 0, whose outline in most fonts is the
    /// "missing glyph" box. Rendering it turned pasted Windows line endings and
    /// tabs into rows of tofu.
    #[test]
    fn control_characters_draw_nothing() {
        use crate::Canvas;
        use bettershot_core::math::Vec2D;
        use bettershot_core::painter::{Painter, TextDraw};
        use bettershot_core::style::Color;

        let ink = |text: &str| {
            let base = Canvas::filled(160, 60, Color::white());
            let mut canvas = base.clone();
            {
                let mut painter = crate::CpuPainter::new(&mut canvas, &base);
                painter.draw_text(&TextDraw::new(
                    Vec2D::new(4.0, 4.0),
                    text,
                    20.0,
                    Color::black(),
                ));
            }
            (0..canvas.height())
                .flat_map(|y| (0..canvas.width()).map(move |x| (x, y)))
                .filter(|(x, y)| canvas.pixel(*x, *y) != Color::white())
                .count()
        };

        let plain = ink("ab");
        assert!(plain > 0, "the control case drew nothing at all");
        for text in ["a\rb", "a\tb", "a\u{7}b"] {
            assert_eq!(
                ink(text),
                plain,
                "{text:?} drew extra ink; a control character is being rendered"
            );
        }
    }

    use super::*;

    fn any_font() -> Font {
        Font::system()
    }

    #[test]
    fn the_fallback_reports_itself() {
        let f = Font::fallback();
        assert!(f.is_fallback());
        assert_eq!(f.source(), &FontSource::Fallback);
    }

    #[test]
    fn an_unparsable_file_is_rejected() {
        let err = Font::from_bytes(b"not a font".to_vec()).unwrap_err();
        assert!(matches!(err, RenderError::FontInvalid { .. }));
        let err = Font::from_path("/definitely/not/here.ttf").unwrap_err();
        assert!(matches!(err, RenderError::Io { .. }));
    }

    #[test]
    fn measurement_grows_with_length_and_with_size() {
        for font in [any_font(), Font::fallback()] {
            let short = font.measure("Hi", 20.0);
            let long = font.measure("Hi there", 20.0);
            let big = font.measure("Hi", 40.0);
            assert!(long.x > short.x, "{:?}", font.source());
            assert!(big.x > short.x && big.y > short.y, "{:?}", font.source());
        }
    }

    #[test]
    fn measurement_of_multiple_lines_stacks_them() {
        for font in [any_font(), Font::fallback()] {
            let one = font.measure("Hi", 20.0);
            let two = font.measure("Hi\nHi", 20.0);
            assert!((two.y - one.y * 2.0).abs() < 0.01);
            assert!((two.x - one.x).abs() < 0.01, "same widest line");
        }
    }

    #[test]
    fn empty_text_measures_to_one_empty_line() {
        let m = any_font().measure("", 20.0);
        assert_eq!(m.x, 0.0);
        assert!((m.y - 24.0).abs() < 0.01);
    }

    #[test]
    fn a_nonsense_size_measures_to_zero_and_draws_nothing() {
        let font = any_font();
        assert_eq!(font.measure("Hi", f32::NAN), Vec2D::ZERO);
        let mut canvas = Canvas::filled(20, 20, Color::white());
        draw_text(
            &mut canvas,
            &font,
            &TextDraw::new(Vec2D::new(2.0, 2.0), "Hi", f32::NAN, Color::black()),
        );
        assert!((0..20).all(|y| (0..20).all(|x| canvas.pixel(x, y) == Color::white())));
    }

    fn painted(canvas: &Canvas) -> usize {
        (0..canvas.height())
            .flat_map(|y| (0..canvas.width()).map(move |x| (x, y)))
            .filter(|(x, y)| canvas.pixel(*x, *y) != Color::white())
            .count()
    }

    #[test]
    fn rendering_text_paints_pixels_with_either_backend() {
        for font in [any_font(), Font::fallback()] {
            let mut canvas = Canvas::filled(120, 60, Color::white());
            draw_text(
                &mut canvas,
                &font,
                &TextDraw::new(Vec2D::new(6.0, 6.0), "Hi", 32.0, Color::black()),
            );
            assert!(painted(&canvas) > 20, "{:?} painted nothing", font.source());
        }
    }

    #[test]
    fn multi_byte_and_emoji_input_does_not_panic() {
        let font = any_font();
        let mut canvas = Canvas::filled(200, 80, Color::white());
        for text in ["héllo", "日本語", "🎉 done", "a\u{0301}\n🇯🇵"] {
            draw_text(
                &mut canvas,
                &font,
                &TextDraw::new(Vec2D::new(4.0, 4.0), text, 24.0, Color::black())
                    .with_cursor(Some(1)),
            );
            let _ = font.measure(text, 24.0);
        }
    }

    #[test]
    fn a_cursor_past_the_end_or_mid_character_is_clamped() {
        let font = any_font();
        let mut canvas = Canvas::filled(80, 40, Color::white());
        for cursor in [0, 1, 2, 3, 999] {
            draw_text(
                &mut canvas,
                &font,
                &TextDraw::new(Vec2D::new(4.0, 4.0), "é", 20.0, Color::black())
                    .with_cursor(Some(cursor)),
            );
        }
        assert!(painted(&canvas) > 0);
    }

    #[test]
    fn the_caret_is_drawn_even_for_empty_text() {
        let font = any_font();
        let mut plain = Canvas::filled(40, 40, Color::white());
        draw_text(
            &mut plain,
            &font,
            &TextDraw::new(Vec2D::new(10.0, 5.0), "", 20.0, Color::black()),
        );
        assert_eq!(painted(&plain), 0, "no glyphs, no caret");

        let mut with_caret = Canvas::filled(40, 40, Color::white());
        draw_text(
            &mut with_caret,
            &font,
            &TextDraw::new(Vec2D::new(10.0, 5.0), "", 20.0, Color::black()).with_cursor(Some(0)),
        );
        assert!(painted(&with_caret) > 0, "the caret should be visible");
    }

    #[test]
    fn the_caret_advances_with_the_byte_offset() {
        let font = any_font();
        let x_of_caret = |cursor: usize| {
            let mut canvas = Canvas::filled(200, 40, Color::white());
            draw_text(
                &mut canvas,
                &font,
                &TextDraw::new(Vec2D::new(5.0, 5.0), "MMMM", 20.0, Color::black())
                    .with_cursor(Some(cursor)),
            );
            // Rightmost painted column.
            (0..200)
                .rfind(|x| (0..40).any(|y| canvas.pixel(*x, y) != Color::white()))
                .unwrap_or(0)
        };
        assert!(x_of_caret(4) > x_of_caret(0));
    }

    #[test]
    fn a_background_plate_covers_the_block() {
        let font = any_font();
        let mut canvas = Canvas::filled(120, 60, Color::white());
        draw_text(
            &mut canvas,
            &font,
            &TextDraw::new(Vec2D::new(20.0, 20.0), "Hi", 24.0, Color::black())
                .with_background(Some(Color::blue())),
        );
        // Just inside the plate, above the glyph tops.
        assert_ne!(canvas.pixel(20, 21), Color::white(), "plate is missing");
        assert_eq!(canvas.pixel(2, 2), Color::white(), "plate must not spread");
    }

    /// The centred anchor is what puts a marker's number in the middle of its
    /// disc, so it gets a dedicated test.
    #[test]
    fn centered_text_is_centred_on_both_axes() {
        for font in [any_font(), Font::fallback()] {
            let mut canvas = Canvas::filled(101, 101, Color::white());
            draw_text(
                &mut canvas,
                &font,
                &TextDraw::new(Vec2D::new(50.0, 50.0), "42", 40.0, Color::black()).centered(),
            );
            let mut min = (u32::MAX, u32::MAX);
            let mut max = (0u32, 0u32);
            for y in 0..101 {
                for x in 0..101 {
                    if canvas.pixel(x, y) != Color::white() {
                        min = (min.0.min(x), min.1.min(y));
                        max = (max.0.max(x), max.1.max(y));
                    }
                }
            }
            assert!(max.0 >= min.0, "nothing was painted");
            let cx = (min.0 + max.0) as f32 / 2.0;
            let cy = (min.1 + max.1) as f32 / 2.0;
            assert!(
                (cx - 50.0).abs() < 6.0,
                "x centre {cx} ({:?})",
                font.source()
            );
            assert!(
                (cy - 50.0).abs() < 8.0,
                "y centre {cy} ({:?})",
                font.source()
            );
        }
    }

    #[test]
    fn text_far_off_canvas_is_clipped_not_panicked() {
        let font = any_font();
        let mut canvas = Canvas::filled(20, 20, Color::white());
        for pos in [
            Vec2D::new(-5000.0, -5000.0),
            Vec2D::new(1.0e9, 1.0e9),
            Vec2D::new(f32::NAN, 0.0),
            Vec2D::new(0.0, f32::INFINITY),
        ] {
            draw_text(
                &mut canvas,
                &font,
                &TextDraw::new(pos, "Hi", 24.0, Color::black()).with_cursor(Some(1)),
            );
        }
        assert_eq!(painted(&canvas), 0);
    }

    #[test]
    fn the_shared_system_font_is_only_built_once() {
        let a = system_font();
        let b = system_font();
        assert!(std::ptr::eq(a, b));
    }
}
