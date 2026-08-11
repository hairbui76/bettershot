//! Render one of every annotation onto a synthetic screenshot and write a PNG,
//! so the output can be eyeballed rather than only asserted on.
//!
//! ```text
//! cargo run -p bettershot-render --example showcase -- /tmp/showcase.png
//! ```
//!
//! Defaults to `target/showcase.png` when no path is given, which keeps the
//! example from writing outside the build directory by accident.

use bettershot_core::math::{Rect, Vec2D};
use bettershot_core::scene::Scene;
use bettershot_core::style::{Color, Size, Style};
use bettershot_core::tools::{
    Arrow, Blur, Brush, Ellipse, Highlight, Line, Marker, ObscureKind, Rectangle, Text,
};
use bettershot_render::{Canvas, RenderError, render_scene, system_font};

const W: u32 = 900;
const H: u32 = 560;

fn main() -> Result<(), RenderError> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/showcase.png".to_string());

    let base = fake_screenshot();
    let scene = showcase_scene();

    let started = std::time::Instant::now();
    let out = render_scene(&base, &scene);
    let elapsed = started.elapsed();

    out.save_png(&path)?;
    println!(
        "wrote {path} ({W}x{H}, {} annotations) in {elapsed:?} using {:?}",
        scene.annotation_count(),
        system_font().source(),
    );
    Ok(())
}

/// Something with enough structure that blur and pixelate are obviously doing
/// their job: a soft gradient with a grid and some "text lines" over it.
fn fake_screenshot() -> Canvas {
    let mut c = Canvas::new(W, H);
    for y in 0..H {
        for x in 0..W {
            let u = x as f32 / W as f32;
            let v = y as f32 / H as f32;
            let mut r = (60.0 + 120.0 * u) as u8;
            let mut g = (70.0 + 90.0 * v) as u8;
            let mut b = (140.0 + 80.0 * (1.0 - u)) as u8;
            if x % 40 == 0 || y % 40 == 0 {
                r = r.saturating_add(35);
                g = g.saturating_add(35);
                b = b.saturating_add(35);
            }
            // Fake lines of text in the lower right, for the redaction demo.
            if (600..880).contains(&x) && (380..520).contains(&y) && (y / 6) % 3 != 2 {
                r = 235;
                g = 235;
                b = 240;
            }
            c.set_pixel(x, y, Color::rgb(r, g, b));
        }
    }
    c
}

fn showcase_scene() -> Scene {
    let mut scene = Scene::new(Vec2D::new(W as f32, H as f32));
    let style = |color: Color, size: Size| Style {
        color,
        size,
        fill: false,
        round_caps: true,
        annotation_size_factor: 0.6,
    };

    scene.add(Box::new(Rectangle {
        rect: Rect::from_xywh(40.0, 40.0, 220.0, 140.0),
        style: style(Color::red(), Size::Medium),
    }));
    scene.add(Box::new(Ellipse {
        rect: Rect::from_xywh(300.0, 40.0, 200.0, 140.0),
        style: style(Color::green(), Size::Large),
    }));
    scene.add(Box::new(Rectangle {
        rect: Rect::from_xywh(540.0, 60.0, 140.0, 100.0),
        style: Style {
            fill: true,
            ..style(Color::blue(), Size::Medium)
        },
    }));
    scene.add(Box::new(Arrow {
        start: Vec2D::new(60.0, 260.0),
        end: Vec2D::new(280.0, 210.0),
        style: Style {
            fill: true,
            ..style(Color::orange(), Size::Large)
        },
    }));
    scene.add(Box::new(Line {
        start: Vec2D::new(60.0, 300.0),
        end: Vec2D::new(280.0, 340.0),
        style: style(Color::pink(), Size::Medium),
    }));

    let mut brush = Brush::new(style(Color::cove(), Size::Medium));
    for i in 0..=60 {
        let t = i as f32 / 60.0;
        brush.push(Vec2D::new(
            330.0 + t * 240.0,
            300.0 + (t * std::f32::consts::TAU).sin() * 45.0,
        ));
    }
    scene.add(Box::new(brush));

    scene.add(Box::new(Highlight {
        rect: Rect::from_xywh(40.0, 400.0, 300.0, 50.0),
        style: style(Color::orange(), Size::Medium),
    }));
    scene.add(Box::new(Text::editing(
        Vec2D::new(52.0, 405.0),
        "highlighted note\nwith a second line",
        style(Color::black(), Size::Medium),
        9,
    )));

    for (i, x) in [740.0f32, 800.0, 860.0].into_iter().enumerate() {
        scene.add(Box::new(Marker {
            pos: Vec2D::new(x, 220.0),
            number: i as u16 + 1,
            style: style(Color::red(), Size::Large),
        }));
    }

    scene.add(Box::new(Blur {
        rect: Rect::from_xywh(600.0, 380.0, 140.0, 140.0),
        style: style(Color::red(), Size::Large),
        obscure: ObscureKind::Blur,
    }));
    scene.add(Box::new(Blur {
        rect: Rect::from_xywh(750.0, 380.0, 130.0, 140.0),
        style: style(Color::red(), Size::Medium),
        obscure: ObscureKind::Pixelate,
    }));

    scene
}
