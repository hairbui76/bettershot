//! Measures render cost and memory at realistic screenshot sizes.
//!
//! Run with: cargo run --release -p bettershot-render --example perf_audit

use bettershot_core::math::{Rect, Vec2D};
use bettershot_core::scene::Scene;
use bettershot_core::style::{Color, Size, Style};
use bettershot_core::tools::{Arrow, Blur, Marker, ObscureKind, Rectangle};
use bettershot_render::{Canvas, render_scene};
use std::time::Instant;

fn scene_for(w: f32, h: f32) -> Scene {
    let mut scene = Scene::new(Vec2D::new(w, h));
    let style = Style {
        color: Color::red(),
        size: Size::Medium,
        ..Default::default()
    };
    for i in 0..10 {
        let x = 40.0 + i as f32 * 60.0;
        scene.add(Box::new(Rectangle {
            rect: Rect::from_xywh(x, 60.0, 120.0, 90.0),
            style,
        }));
        scene.add(Box::new(Arrow {
            start: Vec2D::new(x, 300.0),
            end: Vec2D::new(x + 150.0, 420.0),
            style,
        }));
        scene.add(Box::new(Marker {
            pos: Vec2D::new(x, 500.0),
            number: i + 1,
            style,
        }));
    }
    scene.add(Box::new(Blur {
        rect: Rect::from_xywh(100.0, 600.0, 400.0, 300.0),
        style,
        obscure: ObscureKind::Blur,
    }));
    scene
}

fn main() {
    println!(
        "{:<22} {:>10} {:>12} {:>14}",
        "case", "render", "base bytes", "peak est."
    );
    for (label, w, h) in [
        ("1080p", 1920u32, 1080u32),
        ("1440p", 2560, 1440),
        ("4K", 3840, 2160),
        ("dual 4K stitched", 7680, 2160),
    ] {
        let base = Canvas::filled(w, h, Color::rgb(20, 30, 60));
        let scene = scene_for(w as f32, h as f32);

        let start = Instant::now();
        let out = render_scene(&base, &scene);
        let elapsed = start.elapsed();

        let base_bytes = (w as usize) * (h as usize) * 4;
        // Export holds: the base, the destination, and the app additionally
        // holds one GPU texture of the same size.
        let peak = base_bytes * 3;
        println!(
            "{label:<22} {:>8.1}ms {:>10.1}MB {:>12.1}MB",
            elapsed.as_secs_f64() * 1000.0,
            base_bytes as f64 / 1e6,
            peak as f64 / 1e6,
        );
        std::hint::black_box(out);
    }
}
