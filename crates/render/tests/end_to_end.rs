//! End-to-end coverage: a real [`Scene`] of real drawables, rendered to real
//! pixels, then round-tripped through PNG.
//!
//! These exist because `crates/core` can only prove *what* a tool would draw.
//! Everything from there to the exported file — triangulation, coverage,
//! stroke expansion, glyph placement, base-image sampling — is only ever
//! exercised here.

use bettershot_core::drawable::Drawable;
use bettershot_core::math::{Rect, Vec2D};
use bettershot_core::painter::{ImageEffect, Painter, TextAlign, TextDraw};
use bettershot_core::scene::Scene;
use bettershot_core::style::{Color, Size, Style};
use bettershot_core::tools::{
    Arrow, Blur, Brush, CropOverlay, Ellipse, Highlight, Line, Marker, ObscureKind, Rectangle, Text,
};
use bettershot_render::{Canvas, CpuPainter, Font, RenderError, render_scene};

const W: u32 = 320;
const H: u32 = 240;

/// A deterministic, high-frequency background. High frequency matters: it makes
/// "did the blur do anything" a question with an unambiguous answer.
fn wallpaper() -> Canvas {
    let mut c = Canvas::new(W, H);
    for y in 0..H {
        for x in 0..W {
            let checker = ((x / 2) + (y / 2)) % 2 == 0;
            let color = if checker {
                Color::rgb(20, 40, 200)
            } else {
                Color::rgb(230, 220, 60)
            };
            c.set_pixel(x, y, color);
        }
    }
    c
}

/// A base with no repeating structure at all. `wallpaper` is a fine backdrop
/// for "did anything happen here", but its 2x2 checker averages to almost the
/// same colour over any two blocks, which would let block-alignment bugs pass
/// unnoticed. This one cannot.
fn noise() -> Canvas {
    let mut c = Canvas::new(W, H);
    let mut state = 0x2545_f491_4f6c_dd1du64;
    for y in 0..H {
        for x in 0..W {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let b = state.to_le_bytes();
            c.set_pixel(x, y, Color::new(b[0], b[1], b[2], 255));
        }
    }
    c
}

fn style() -> Style {
    Style {
        color: Color::red(),
        size: Size::Medium,
        fill: false,
        round_caps: true,
        annotation_size_factor: 1.0,
    }
}

/// The scene used by most of the tests below. Regions, by design, do not
/// overlap, so each assertion isolates one drawable.
///
/// * rectangle outline   (10,10)-(90,70)
/// * arrow               (120,20) -> (200,60)
/// * brush stroke        around (30,120)-(90,160)
/// * marker "1"          at (250,40)
/// * highlight           (120,110)-(220,150)
/// * blur                (240,150)-(310,220)
fn demo_scene() -> Scene {
    let mut scene = Scene::new(Vec2D::new(W as f32, H as f32));

    scene.add(Box::new(Rectangle {
        rect: Rect::from_xywh(10.0, 10.0, 80.0, 60.0),
        style: style(),
    }));
    scene.add(Box::new(Arrow {
        start: Vec2D::new(120.0, 20.0),
        end: Vec2D::new(200.0, 60.0),
        style: style().with_color(Color::green()),
    }));
    let mut brush = Brush::new(style().with_color(Color::cove()));
    for i in 0..=8 {
        let t = i as f32 / 8.0;
        brush.push(Vec2D::new(
            30.0 + t * 60.0,
            120.0 + (t * 6.0).sin() * 18.0 + 18.0,
        ));
    }
    scene.add(Box::new(brush));
    scene.add(Box::new(Marker {
        pos: Vec2D::new(250.0, 40.0),
        number: 1,
        style: style().with_color(Color::pink()),
    }));
    scene.add(Box::new(Highlight {
        rect: Rect::from_xywh(120.0, 110.0, 100.0, 40.0),
        style: style().with_color(Color::orange()),
    }));
    scene.add(Box::new(Blur {
        rect: Rect::from_xywh(240.0, 150.0, 70.0, 70.0),
        style: style(),
        obscure: ObscureKind::Blur,
    }));

    scene
}

/// Count pixels that differ between two canvases inside a rect.
fn changed(a: &Canvas, b: &Canvas, rect: Rect) -> usize {
    let (x0, y0) = (rect.left() as u32, rect.top() as u32);
    let (x1, y1) = (rect.right() as u32, rect.bottom() as u32);
    (y0..y1)
        .flat_map(|y| (x0..x1).map(move |x| (x, y)))
        .filter(|(x, y)| a.pixel(*x, *y) != b.pixel(*x, *y))
        .count()
}

/// Mean absolute difference between horizontally adjacent red channels.
fn roughness(canvas: &Canvas, rect: Rect) -> f32 {
    let (x0, y0) = (rect.left() as u32, rect.top() as u32);
    let (x1, y1) = (rect.right() as u32, rect.bottom() as u32);
    let mut total = 0.0f32;
    let mut n = 0.0f32;
    for y in y0..y1 {
        for x in x0..x1 - 1 {
            total += (canvas.pixel(x, y).r as f32 - canvas.pixel(x + 1, y).r as f32).abs();
            n += 1.0;
        }
    }
    total / n.max(1.0)
}

#[test]
fn every_annotation_lands_in_its_own_region() {
    let base = wallpaper();
    let out = render_scene(&base, &demo_scene());
    assert_eq!(out.width(), W);
    assert_eq!(out.height(), H);

    let regions = [
        ("rectangle", Rect::from_xywh(8.0, 8.0, 84.0, 64.0)),
        ("arrow", Rect::from_xywh(115.0, 15.0, 90.0, 50.0)),
        ("brush", Rect::from_xywh(25.0, 115.0, 70.0, 50.0)),
        ("marker", Rect::from_xywh(225.0, 15.0, 50.0, 50.0)),
        ("highlight", Rect::from_xywh(120.0, 110.0, 100.0, 40.0)),
        ("blur", Rect::from_xywh(240.0, 150.0, 70.0, 70.0)),
    ];
    for (name, rect) in regions {
        assert!(
            changed(&base, &out, rect) > 40,
            "{name} should have painted something in {rect:?}"
        );
    }
}

#[test]
fn the_gaps_between_annotations_are_untouched() {
    let base = wallpaper();
    let out = render_scene(&base, &demo_scene());

    // Strips that no drawable in `demo_scene` reaches.
    for rect in [
        Rect::from_xywh(0.0, 0.0, 6.0, H as f32),
        Rect::from_xywh(0.0, 200.0, 100.0, 40.0),
        Rect::from_xywh(100.0, 180.0, 100.0, 60.0),
        Rect::from_xywh(95.0, 0.0, 20.0, 100.0),
    ] {
        assert_eq!(changed(&base, &out, rect), 0, "{rect:?} should be pristine");
    }
}

#[test]
fn the_highlight_tints_without_hiding_the_background() {
    let base = wallpaper();
    let out = render_scene(&base, &demo_scene());

    // Highlight is a translucent fill: every pixel changes, but the underlying
    // checkerboard contrast must survive.
    let rect = Rect::from_xywh(125.0, 115.0, 90.0, 30.0);
    assert_eq!(changed(&base, &out, rect), (90 * 30) as usize);
    assert!(
        roughness(&out, rect) > 30.0,
        "the highlight must not be opaque"
    );
}

#[test]
fn the_blur_hides_the_background_and_ignores_what_was_drawn_over_it() {
    let base = wallpaper();
    let mut scene = demo_scene();
    // Draw an opaque rectangle *under* the blur, after everything else. The
    // blur is committed earlier, but effects always re-sample the base, so the
    // blurred output must not contain any trace of this rectangle.
    scene.add(Box::new(Rectangle {
        rect: Rect::from_xywh(245.0, 155.0, 60.0, 60.0),
        style: style().with_color(Color::black()).with_fill(true),
    }));

    // Scene order is: ... blur, then rectangle. The rectangle wins where it is
    // drawn, so blur a *fresh* scene to compare against.
    let mut blur_only = Scene::new(base.size());
    blur_only.add(Box::new(Blur {
        rect: Rect::from_xywh(240.0, 150.0, 70.0, 70.0),
        style: style(),
        obscure: ObscureKind::Blur,
    }));
    let blurred = render_scene(&base, &blur_only);

    let inner = Rect::from_xywh(250.0, 160.0, 50.0, 50.0);
    assert!(
        roughness(&base, inner) > 100.0,
        "the base is a hard checkerboard"
    );
    assert!(
        roughness(&blurred, inner) < 25.0,
        "blur should smooth it, got {}",
        roughness(&blurred, inner)
    );

    // Now the ordering check: rectangle first, blur second, same scene.
    let mut ordered = Scene::new(base.size());
    ordered.add(Box::new(Rectangle {
        rect: Rect::from_xywh(240.0, 150.0, 70.0, 70.0),
        style: style().with_color(Color::black()).with_fill(true),
    }));
    ordered.add(Box::new(Blur {
        rect: Rect::from_xywh(240.0, 150.0, 70.0, 70.0),
        style: style(),
        obscure: ObscureKind::Blur,
    }));
    let ordered_out = render_scene(&base, &ordered);
    for (x, y) in [(260, 170), (280, 190), (300, 210)] {
        assert_eq!(
            ordered_out.pixel(x, y),
            blurred.pixel(x, y),
            "({x},{y}) must show the blurred base, not the black rectangle"
        );
    }
}

#[test]
fn pixelate_blocks_are_flat_end_to_end() {
    let base = wallpaper();
    let mut scene = Scene::new(base.size());
    scene.add(Box::new(Blur {
        rect: Rect::from_xywh(40.0, 40.0, 80.0, 80.0),
        style: style().with_size(Size::Large),
        obscure: ObscureKind::Pixelate,
    }));
    let out = render_scene(&base, &scene);

    // Size::Large -> block_size 30, and blocks tile from the *image* origin, so
    // this rect starts part-way into the block spanning 30..60.
    let expected = out.pixel(40, 40);
    for y in 40..60 {
        for x in 40..60 {
            assert_eq!(out.pixel(x, y), expected, "block pixel ({x},{y})");
        }
    }
    // The next grid line is at 60, not at rect.left() + 30.
    for y in 60..90 {
        for x in 60..90 {
            assert_eq!(out.pixel(x, y), out.pixel(60, 60), "block pixel ({x},{y})");
        }
    }
    // The mosaic flattens the region completely; the base is maximally rough.
    let inner = Rect::from_xywh(41.0, 41.0, 18.0, 18.0);
    assert!(roughness(&base, inner) > 100.0);
    assert_eq!(roughness(&out, inner), 0.0);
    assert_ne!(expected, base.pixel(40, 40), "the block is an average");
    assert_eq!(out.pixel(39, 39), base.pixel(39, 39), "outside the rect");
}

/// The editor previews a redaction by pre-processing the whole base image into
/// a texture; export re-runs the effect over the annotation's rect. For a tool
/// whose job includes hiding sensitive information, those two must be the same
/// pixels — otherwise the user approves one redaction and ships another.
///
/// This is the end-to-end statement of that: a real `Blur` drawable, through a
/// real `Scene`, compared byte-for-byte against the whole-image call the app
/// makes.
#[track_caller]
fn assert_preview_matches_export(base: &Canvas, rect: Rect, obscure: ObscureKind, size: Size) {
    let drawable = Blur {
        rect,
        style: style().with_size(size),
        obscure,
    };
    let effect = drawable.effect();
    let mut scene = Scene::new(base.size());
    scene.add(Box::new(drawable));

    let exported = render_scene(base, &scene);
    let preview = bettershot_render::apply_effect(base, effect);

    let x0 = (rect.left().floor() as u32).min(base.width());
    let y0 = (rect.top().floor() as u32).min(base.height());
    let x1 = (rect.right().ceil() as u32).min(base.width());
    let y1 = (rect.bottom().ceil() as u32).min(base.height());
    assert!(x1 > x0 && y1 > y0, "{rect:?} must cover something");

    for y in 0..base.height() {
        for x in 0..base.width() {
            let covered = (x0..x1).contains(&x) && (y0..y1).contains(&y);
            let want = if covered { &preview } else { base };
            assert_eq!(
                exported.pixel(x, y),
                want.pixel(x, y),
                "({x},{y}) covered={covered} for {obscure:?} {size} over {rect:?}",
            );
        }
    }
    assert!(
        changed(
            base,
            &exported,
            Rect::from_xywh(x0 as f32, y0 as f32, (x1 - x0) as f32, (y1 - y0) as f32)
        ) > 0,
        "{obscure:?} over {rect:?} did not redact anything",
    );
}

#[test]
fn a_previewed_blur_is_byte_for_byte_what_gets_exported() {
    let base = noise();
    for size in Size::ALL {
        for rect in [
            // Middle of the image.
            Rect::from_xywh(100.0, 80.0, 90.0, 70.0),
            // Touching each edge, where clamp-to-edge sampling takes over.
            Rect::from_xywh(0.0, 0.0, 80.0, 60.0),
            Rect::from_xywh(W as f32 - 80.0, H as f32 - 60.0, 80.0, 60.0),
            Rect::from_xywh(0.0, 100.0, W as f32, 40.0),
            // The whole image.
            Rect::from_xywh(0.0, 0.0, W as f32, H as f32),
            // Sub-pixel geometry, which clips outwards to whole pixels.
            Rect::from_xywh(60.25, 40.5, 70.75, 50.5),
        ] {
            assert_preview_matches_export(&base, rect, ObscureKind::Blur, size);
        }
    }
}

#[test]
fn a_previewed_pixelation_is_byte_for_byte_what_gets_exported() {
    let base = noise();
    for size in Size::ALL {
        for rect in [
            // Deliberately off the block grid: block colours must come from the
            // image's tiling, not from wherever the drag started.
            Rect::from_xywh(103.0, 77.0, 91.0, 73.0),
            Rect::from_xywh(0.0, 0.0, 80.0, 60.0),
            Rect::from_xywh(W as f32 - 77.0, H as f32 - 53.0, 77.0, 53.0),
            Rect::from_xywh(0.0, 0.0, W as f32, H as f32),
            Rect::from_xywh(60.25, 40.5, 70.75, 50.5),
        ] {
            assert_preview_matches_export(&base, rect, ObscureKind::Pixelate, size);
        }
    }
}

#[test]
fn adjacent_redactions_share_one_block_grid() {
    // Two pixelations dragged over neighbouring parts of the same line must not
    // produce two mosaics that visibly disagree along the seam.
    let base = noise();
    let mut scene = Scene::new(base.size());
    for left in [40.0, 97.0] {
        scene.add(Box::new(Blur {
            rect: Rect::from_xywh(left, 40.0, 57.0, 40.0),
            style: style().with_size(Size::Medium),
            obscure: ObscureKind::Pixelate,
        }));
    }
    let out = render_scene(&base, &scene);

    // Size::Medium -> block_size 20, so the seam at x = 97 falls inside the
    // block spanning 80..100, which must still be one flat colour.
    for y in 40..60 {
        for x in 80..100 {
            assert_eq!(out.pixel(x, y), out.pixel(80, 40), "at ({x},{y})");
        }
    }
}

#[test]
fn the_rendered_image_survives_a_png_round_trip() {
    let out = render_scene(&wallpaper(), &demo_scene());
    let bytes = out.encode_png().unwrap();
    let back = Canvas::decode_png(&bytes).unwrap();
    assert_eq!(back, out, "PNG must be lossless for RGBA8");
    assert_eq!(Canvas::decode(&bytes).unwrap(), out);
}

#[test]
fn the_rendered_image_survives_a_file_round_trip() {
    // `CARGO_TARGET_TMPDIR` is inside the target directory, so this test never
    // writes anywhere it should not.
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("round_trip.png");
    let out = render_scene(&wallpaper(), &demo_scene());
    out.save_png(&path).unwrap();
    assert_eq!(Canvas::load_png(&path).unwrap(), out);
    assert_eq!(Canvas::load(&path).unwrap(), out);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn saving_to_an_unwritable_path_is_an_error_not_a_panic() {
    let out = Canvas::new(2, 2);
    let err = out
        .save_png("/definitely/not/a/directory/x.png")
        .unwrap_err();
    assert!(matches!(err, RenderError::Io { .. }), "{err:?}");
}

#[test]
fn undo_removes_an_annotation_from_the_render() {
    let base = wallpaper();
    let mut scene = demo_scene();
    let before = render_scene(&base, &scene);
    assert!(scene.undo(), "the blur should be undoable");
    let after = render_scene(&base, &scene);

    assert_ne!(before, after);
    assert_eq!(
        changed(&base, &after, Rect::from_xywh(240.0, 150.0, 70.0, 70.0)),
        0,
        "the undone blur must leave no trace"
    );
    // Everything committed earlier is still there.
    assert!(changed(&base, &after, Rect::from_xywh(8.0, 8.0, 84.0, 64.0)) > 40);
}

#[test]
fn a_crop_rebases_annotations_into_the_cropped_image() {
    let base = wallpaper();
    let mut scene = Scene::new(base.size());
    scene.add(Box::new(Rectangle {
        rect: Rect::from_xywh(100.0, 100.0, 40.0, 40.0),
        style: style().with_fill(true),
    }));
    scene.apply_crop(Rect::from_xywh(80.0, 80.0, 160.0, 120.0));

    // The caller crops the base itself; here we fake it with a smaller canvas.
    let cropped_base = Canvas::filled(160, 120, Color::white());
    let out = render_scene(&cropped_base, &scene);
    assert_eq!(out.pixel(25, 25), Color::red(), "rect moved to (20,20)");
    assert_eq!(out.pixel(5, 5), Color::white());
}

#[test]
fn a_full_scene_of_every_drawable_kind_renders_without_panicking() {
    let base = wallpaper();
    let mut scene = Scene::new(base.size());
    scene.add(Box::new(Rectangle {
        rect: Rect::from_xywh(5.0, 5.0, 50.0, 40.0),
        style: style().with_fill(true),
    }));
    scene.add(Box::new(Ellipse {
        rect: Rect::from_xywh(60.0, 5.0, 50.0, 40.0),
        style: style(),
    }));
    scene.add(Box::new(Line {
        start: Vec2D::new(5.0, 60.0),
        end: Vec2D::new(110.0, 95.0),
        style: style(),
    }));
    scene.add(Box::new(Arrow {
        start: Vec2D::new(120.0, 60.0),
        end: Vec2D::new(220.0, 95.0),
        style: style().with_fill(true),
    }));
    let mut typing = Text::editing(Vec2D::new(10.0, 150.0), "hello\nwörld 🎉", style(), 3);
    typing.preedit = "ime".to_string();
    scene.add(Box::new(typing));
    scene.add(Box::new(Marker {
        pos: Vec2D::new(280.0, 30.0),
        number: 42,
        style: style(),
    }));
    scene.add(Box::new(CropOverlay {
        rect: Rect::from_xywh(20.0, 20.0, 280.0, 200.0),
        canvas: base.bounds(),
    }));

    let out = render_scene(&base, &scene);
    assert_ne!(out, base);
    assert_eq!(out.width(), W);
}

#[test]
fn a_marker_puts_its_number_inside_its_disc() {
    let base = Canvas::filled(120, 120, Color::white());
    let mut scene = Scene::new(base.size());
    let marker = Marker {
        pos: Vec2D::new(60.0, 60.0),
        number: 7,
        // White disc, so the label's contrast colour is black and easy to find.
        style: style().with_color(Color::white()),
    };
    let radius = marker.radius();
    scene.add(Box::new(marker));
    let out = render_scene(&base, &scene);

    let label: Vec<(u32, u32)> = (0..120)
        .flat_map(|y| (0..120).map(move |x| (x, y)))
        .filter(|(x, y)| out.pixel(*x, *y).luminance() < 0.2)
        .collect();
    assert!(!label.is_empty(), "the number should be drawn in black");
    for (x, y) in &label {
        let d = Vec2D::new(*x as f32, *y as f32).distance_to(&Vec2D::new(60.0, 60.0));
        assert!(
            d < radius,
            "label pixel ({x},{y}) escaped the disc (r={radius})"
        );
    }
}

#[test]
fn measured_text_matches_what_is_painted() {
    let base = Canvas::filled(400, 120, Color::white());
    let mut canvas = base.clone();
    let mut painter = CpuPainter::new(&mut canvas, &base);
    let measured = painter.measure_text("Measure me", 36.0);
    painter.draw_text(&TextDraw::new(
        Vec2D::new(20.0, 20.0),
        "Measure me",
        36.0,
        Color::black(),
    ));

    let mut right = 0u32;
    let mut bottom = 0u32;
    for y in 0..120 {
        for x in 0..400 {
            if canvas.pixel(x, y) != Color::white() {
                right = right.max(x);
                bottom = bottom.max(y);
            }
        }
    }
    assert!(right > 20, "nothing was drawn");
    // Ink must fit inside the measured block (advance widths include side
    // bearings, so the ink is never wider than the measurement).
    assert!(
        (right as f32) <= 20.0 + measured.x + 1.0,
        "ink right edge {right} exceeds measured width {}",
        measured.x
    );
    assert!((bottom as f32) <= 20.0 + measured.y + 1.0);
}

#[test]
fn the_block_glyph_fallback_still_produces_readable_geometry() {
    let base = Canvas::filled(200, 80, Color::white());
    let font = Font::fallback();
    assert!(font.is_fallback());

    let mut scene = Scene::new(base.size());
    scene.add(Box::new(Text::committed(
        Vec2D::new(10.0, 10.0),
        "abc",
        style(),
    )));
    let out = bettershot_render::render_scene_with_font(&base, &scene, &font);
    assert_ne!(out, base, "the fallback must still paint something");
}

#[test]
fn centered_and_left_aligned_text_differ_by_half_the_block() {
    let base = Canvas::filled(300, 120, Color::white());

    let ink_left = |align: TextAlign| {
        let mut canvas = base.clone();
        {
            let mut p = CpuPainter::new(&mut canvas, &base);
            let mut draw = TextDraw::new(Vec2D::new(150.0, 60.0), "WWW", 30.0, Color::black());
            draw.align = align;
            p.draw_text(&draw);
        }
        (0..300)
            .find(|x| (0..120).any(|y| canvas.pixel(*x, y) != Color::white()))
            .expect("something must be drawn")
    };

    let left = ink_left(TextAlign::Left);
    let centered = ink_left(TextAlign::Center);
    assert!(centered < left, "centred text starts further left");

    let base_font = bettershot_render::system_font();
    let half = base_font.measure("WWW", 30.0).x / 2.0;
    assert!(
        ((left - centered) as f32 - half).abs() < 4.0,
        "expected a shift of about {half}, got {}",
        left - centered
    );
}

/// A rough guard against accidental algorithmic blow-ups. Generous by design:
/// this runs in debug builds on shared CI machines.
#[test]
fn a_full_hd_render_with_many_annotations_is_not_absurdly_slow() {
    let base = Canvas::filled(1920, 1080, Color::rgb(40, 44, 52));
    let mut scene = Scene::new(base.size());
    for i in 0..30 {
        let x = 20.0 + (i % 6) as f32 * 300.0;
        let y = 20.0 + (i / 6) as f32 * 200.0;
        scene.add(Box::new(Rectangle {
            rect: Rect::from_xywh(x, y, 250.0, 150.0),
            style: style(),
        }));
        scene.add(Box::new(Arrow {
            start: Vec2D::new(x, y + 160.0),
            end: Vec2D::new(x + 240.0, y + 190.0),
            style: style(),
        }));
        scene.add(Box::new(Marker {
            pos: Vec2D::new(x + 40.0, y + 40.0),
            number: i as u16 + 1,
            style: style(),
        }));
    }
    scene.add(Box::new(Blur {
        rect: Rect::from_xywh(600.0, 400.0, 500.0, 300.0),
        style: style(),
        obscure: ObscureKind::Blur,
    }));

    let start = std::time::Instant::now();
    let out = render_scene(&base, &scene);
    let elapsed = start.elapsed();
    assert_ne!(out, base);
    assert!(
        elapsed < std::time::Duration::from_secs(20),
        "1920x1080 with 90 annotations took {elapsed:?}"
    );
}

#[test]
fn hostile_scenes_cannot_take_down_a_render() {
    let base = wallpaper();
    let mut scene = Scene::new(base.size());
    let nasty = [
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        1.0e30,
        -1.0e30,
        0.0,
    ];
    for v in nasty {
        scene.add(Box::new(Rectangle {
            rect: Rect::from_xywh(v, v, v, v),
            style: style(),
        }));
        scene.add(Box::new(Ellipse {
            rect: Rect::from_xywh(v, 10.0, 20.0, v),
            style: style().with_fill(true),
        }));
        scene.add(Box::new(Arrow {
            start: Vec2D::new(v, 0.0),
            end: Vec2D::new(0.0, v),
            style: style(),
        }));
        scene.add(Box::new(Blur {
            rect: Rect::from_xywh(v, v, 30.0, 30.0),
            style: style(),
            obscure: ObscureKind::Pixelate,
        }));
        // A caret offset of 2 lands inside the emoji; `editing` snaps it to a
        // character boundary rather than panicking on the next edit.
        scene.add(Box::new(Text::editing(
            Vec2D::new(v, v),
            "🙈\nnope",
            style(),
            2,
        )));
        let mut brush = Brush::new(style());
        brush.push(Vec2D::new(v, 5.0));
        brush.push(Vec2D::new(5.0, v));
        scene.add(Box::new(brush));
    }
    // The only requirement is that this returns.
    let out = render_scene(&base, &scene);
    assert_eq!(out.width(), W);
    assert_eq!(out.height(), H);
}

#[test]
fn a_zero_sized_base_renders_to_a_zero_sized_output() {
    let base = Canvas::new(0, 0);
    let mut scene = Scene::new(Vec2D::new(100.0, 100.0));
    scene.add(Box::new(Rectangle {
        rect: Rect::from_xywh(0.0, 0.0, 50.0, 50.0),
        style: style().with_fill(true),
    }));
    let out = render_scene(&base, &scene);
    assert!(out.is_empty());
}

#[test]
fn drawables_report_bounds_that_actually_contain_their_ink() {
    let base = Canvas::filled(200, 200, Color::white());
    let cases: Vec<Box<dyn Drawable>> = vec![
        Box::new(Rectangle {
            rect: Rect::from_xywh(30.0, 30.0, 80.0, 60.0),
            style: style(),
        }),
        Box::new(Ellipse {
            rect: Rect::from_xywh(40.0, 40.0, 90.0, 70.0),
            style: style(),
        }),
        Box::new(Line {
            start: Vec2D::new(20.0, 150.0),
            end: Vec2D::new(170.0, 180.0),
            style: style(),
        }),
        Box::new(Marker {
            pos: Vec2D::new(100.0, 100.0),
            number: 3,
            style: style(),
        }),
    ];

    for drawable in cases {
        let mut scene = Scene::new(base.size());
        let bounds = drawable.bounds().expect("bounded drawable").normalized();
        let kind = drawable.kind();
        scene.add(drawable);
        let out = render_scene(&base, &scene);

        // Slack for the anti-aliased fringe and, for markers, glyph overshoot.
        let allowed = bounds.expanded(2.0);
        for y in 0..200u32 {
            for x in 0..200u32 {
                if out.pixel(x, y) != base.pixel(x, y) {
                    assert!(
                        allowed.contains(Vec2D::new(x as f32, y as f32)),
                        "{kind} painted ({x},{y}) outside its bounds {bounds:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn an_image_effect_on_an_annotated_canvas_replaces_rather_than_blends() {
    let base = Canvas::filled(60, 60, Color::rgb(100, 100, 100));
    let mut canvas = base.clone();
    {
        let mut p = CpuPainter::new(&mut canvas, &base);
        p.fill_rect(Rect::from_xywh(0.0, 0.0, 60.0, 60.0), Color::red());
        p.image_effect(
            Rect::from_xywh(10.0, 10.0, 20.0, 20.0),
            ImageEffect::Pixelate { block_size: 5.0 },
        );
    }

    assert_eq!(canvas.pixel(15, 15), Color::rgb(100, 100, 100), "replaced");
    assert_eq!(
        canvas.pixel(50, 50),
        Color::red(),
        "untouched by the effect"
    );
}

// --- Performance regression guards -----------------------------------------
//
// docs/performance.md records numbers that were measured once, by hand, on one
// machine. That is enough to characterise the program and useless as a guard:
// nothing notices when a change makes an operation ten times more expensive.
//
// These assert *ratios* between two measurements taken on the same machine in
// the same run, not wall-clock budgets. A ratio means the same thing on a fast
// laptop and a noisy shared CI runner, where an absolute threshold either
// flakes or is set so loose it catches nothing. Each takes the minimum of
// several runs, because noise can only ever add time.

/// Repeat `f` and return the fastest run.
fn fastest<T>(runs: usize, mut f: impl FnMut() -> T) -> std::time::Duration {
    let mut best = std::time::Duration::MAX;
    for _ in 0..runs {
        let start = std::time::Instant::now();
        std::hint::black_box(f());
        best = best.min(start.elapsed());
    }
    best
}

/// The blur is a three-pass sliding-window box filter, so it is O(1) per pixel
/// per pass *regardless of radius* — see docs/performance.md. A naive kernel
/// would be O(r²) instead, and nothing else in the suite would notice: the
/// output is still a blur, just one that freezes the editor on a large
/// redaction.
#[test]
fn blur_cost_does_not_grow_with_the_radius() {
    let base = Canvas::filled(640, 360, Color::rgb(90, 110, 130));

    let small = fastest(3, || {
        bettershot_render::apply_effect(&base, ImageEffect::Blur { radius: 4.0 })
    });
    let large = fastest(3, || {
        bettershot_render::apply_effect(&base, ImageEffect::Blur { radius: 64.0 })
    });

    // A 16x radius increase would be a 256x cost increase if it were O(r²).
    // Allowing 4x leaves plenty of room for cache effects and scheduling noise
    // while still catching that by two orders of magnitude.
    let ratio = large.as_secs_f64() / small.as_secs_f64().max(1e-9);
    assert!(
        ratio < 4.0,
        "blur at radius 64 took {ratio:.1}x the time of radius 4 \
         ({large:?} vs {small:?}); the sliding-window filter should make radius \
         almost free, so this suggests it became kernel-sized"
    );
}

/// Pixelation is a block average, so it too should not care how big the blocks
/// are.
#[test]
fn pixelate_cost_does_not_grow_with_the_block_size() {
    let base = Canvas::filled(640, 360, Color::rgb(20, 160, 90));

    let small = fastest(3, || {
        bettershot_render::apply_effect(&base, ImageEffect::Pixelate { block_size: 4.0 })
    });
    let large = fastest(3, || {
        bettershot_render::apply_effect(&base, ImageEffect::Pixelate { block_size: 64.0 })
    });

    let ratio = large.as_secs_f64() / small.as_secs_f64().max(1e-9);
    assert!(
        ratio < 4.0,
        "pixelate with 64px blocks took {ratio:.1}x the time of 4px blocks \
         ({large:?} vs {small:?})"
    );
}

/// Export cost should track the pixel count. 4K holds four times as many
/// pixels as 1080p, so anything much beyond that ratio means an operation has
/// gone superlinear in the image size — which is exactly the shape of bug that
/// only shows up on the largest monitor someone owns.
#[test]
fn export_cost_scales_with_the_pixel_count_and_no_worse() {
    fn export(width: u32, height: u32) -> std::time::Duration {
        let base = Canvas::filled(width, height, Color::rgb(40, 44, 52));
        let mut scene = Scene::new(base.size());
        scene.add(Box::new(Rectangle {
            rect: Rect::from_xywh(20.0, 20.0, width as f32 / 2.0, height as f32 / 2.0),
            style: style(),
        }));
        fastest(2, || {
            render_scene(&base, &scene)
                .encode_png()
                .expect("a rendered canvas should encode")
        })
    }

    let hd = export(1920, 1080);
    let uhd = export(3840, 2160);

    // Exactly 4x the pixels. 8x allows for per-pixel work that is not perfectly
    // linear (compression ratios, cache behaviour) without admitting a
    // quadratic.
    let ratio = uhd.as_secs_f64() / hd.as_secs_f64().max(1e-9);
    assert!(
        ratio < 8.0,
        "4K export took {ratio:.1}x the time of 1080p ({uhd:?} vs {hd:?}) for \
         4x the pixels; something is superlinear in the image size"
    );
}
