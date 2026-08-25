// Windows opens a console window for a console-subsystem binary, every launch.
// bettershot is a GUI program, so that terminal is pure noise -- and it appears
// in the user's own screenshots, which for a screenshot tool is worse than
// noise. Building for the windows subsystem stops it.
//
// The cost is that a GUI-subsystem process starts with no standard handles at
// all, so `--help`, `--man`, shell completions and `--output-filename -` would
// write into nothing when run from a terminal. `platform::attach_parent_console`
// puts those back when there is a parent console to attach to.
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

//! bettershot — cross-platform screenshot capture and annotation.
//!
//! `main` owns the process: parse arguments, resolve the configuration, obtain
//! an image (from a file, from stdin, or by capturing the screen), then hand it
//! to the app. Keeping acquisition out of the editor is what lets the same
//! editor serve a piped screenshot and a live capture, and is what will make
//! macOS support additive rather than invasive.

mod app;
mod capture;
mod crash;
mod daemon;
mod editor;
mod effects;
mod egui_painter;
mod history;
mod i18n;
mod notify;
mod output;
mod overlay;
mod platform;
mod settings;
mod view;

use std::io::Write;

use anyhow::{Context, Result, bail};
use bettershot_cli::{Args, InputSource};
use bettershot_core::config::Config;
use clap::Parser;
use image::RgbaImage;

use app::BettershotApp;
use capture::non_empty;

/// When the process started, for the startup diagnostic below. Set as early as
/// possible so the number includes argument parsing and configuration loading,
/// not just the window.
static STARTED: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

/// Elapsed time since the process began, or `None` before `main` ran.
pub(crate) fn since_start() -> Option<std::time::Duration> {
    STARTED.get().map(|t| t.elapsed())
}

fn main() -> Result<()> {
    // Before anything can print: see the note at the top of this file.
    platform::attach_parent_console();

    let _ = STARTED.set(std::time::Instant::now());
    let args = Args::parse();
    env_logger::Builder::new()
        .filter_level(args.log_level())
        .parse_default_env()
        .init();

    if args.is_meta_request() {
        return print_meta(&args);
    }

    let config = bettershot_cli::load_config(&args).context("loading configuration")?;
    config.validate().context("validating configuration")?;
    crash::install(config.crash_reports);

    let app = build_app(&args, config)?;
    run(app)
}

/// Handle the flags that print something and exit: `--man`, `--license` and
/// `--completions`. These exist because `cargo install` delivers no packaged
/// manpage or completion files.
fn print_meta(args: &Args) -> Result<()> {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    if args.man {
        let page = bettershot_cli::render_manpage().context("rendering the manpage")?;
        out.write_all(&page)?;
    }
    if args.license {
        out.write_all(bettershot_cli::LICENSE_TEXT.as_bytes())?;
    }
    if let Some(shell) = args.completions {
        bettershot_cli::render_completions(shell, &mut out);
    }
    out.flush()?;
    Ok(())
}

/// Decide what the app starts as: resident and waiting, the editor for an
/// image we already have, or the selection overlay for a capture that still
/// needs a region chosen.
fn build_app(args: &Args, config: Config) -> Result<BettershotApp> {
    // Daemon mode outranks everything: it is a request to stay running, not to
    // do one thing and exit.
    if config.daemon.enabled {
        // No window until a hotkey or the menu bar asks for one, so on macOS
        // this should not sit in the Dock.
        platform::become_background_app();
        return BettershotApp::idle(config).map_err(|e| anyhow::anyhow!(e));
    }

    match bettershot_cli::input_source(args, &config)? {
        InputSource::File(path) => {
            let image =
                image::open(&path).with_context(|| format!("opening {}", path.display()))?;
            Ok(BettershotApp::editing(config, non_empty(image.to_rgba8())?))
        }
        InputSource::Stdin => {
            let image = read_stdin_image()?;
            Ok(BettershotApp::editing(config, image))
        }
        InputSource::Capture(mode) => {
            let acquired = capture::acquire(mode, &config)?;
            Ok(BettershotApp::stage_for(&config, acquired))
        }
    }
}

fn read_stdin_image() -> Result<RgbaImage> {
    use std::io::Read;
    let mut buffer = Vec::new();
    std::io::stdin()
        .read_to_end(&mut buffer)
        .context("reading the image from stdin")?;
    if buffer.is_empty() {
        bail!("no image data arrived on stdin");
    }
    let image = image::load_from_memory(&buffer).context("decoding the image from stdin")?;
    non_empty(image.to_rgba8())
}

fn run(app: BettershotApp) -> Result<()> {
    let starts_fullscreen = app.starts_fullscreen();
    let starts_on_top = app.starts_on_top();
    let starts_hidden = app.starts_hidden();
    let size = app.initial_size();

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("bettershot")
        .with_inner_size(size);

    // The window and task-bar icon. Embedded rather than read from an icon
    // theme so a portable build unpacked from a zip still shows it. A failure
    // here costs an icon, never the window.
    match eframe::icon_data::from_png_bytes(include_bytes!(
        "../../../assets/icons/bettershot-256.png"
    )) {
        Ok(icon) => viewport = viewport.with_icon(icon),
        Err(e) => log::warn!("the embedded window icon will not decode: {e}"),
    }

    if starts_fullscreen {
        // The selection overlay must cover everything, with no chrome.
        viewport = viewport.with_fullscreen(true).with_decorations(false);
    }
    if starts_on_top {
        // Like the Windows Snipping Tool: a capture is nearly always annotated
        // *about* something else on screen, so the window has to stay in front
        // of what it is describing. A request, not a guarantee — Wayland
        // compositors are free to refuse it.
        viewport = viewport.with_always_on_top();
    }
    if starts_hidden {
        // Daemon mode: resident but invisible until a hotkey fires.
        viewport = viewport.with_visible(false);
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "bettershot",
        options,
        // Inside the creation callback, not before `run_native`: the event loop
        // exists by the time this runs, which is what `global-hotkey` and
        // `tray-icon` both require on Windows.
        Box::new(move |_cc| {
            let mut app = app;
            if !app.start_daemon() {
                return Err("nothing could start a capture; see the notification".into());
            }
            Ok(Box::new(app) as Box<dyn eframe::App>)
        }),
    )
    .map_err(|e| anyhow::anyhow!("could not start the window: {e}"))
}

/// Open large enough to show the screenshot, but never larger than a sensible
/// desktop; the editor scales the image down to fit whatever it gets.
pub(crate) fn initial_window_size([w, h]: [f32; 2]) -> [f32; 2] {
    const MAX: f32 = 1600.0;
    const MIN: f32 = 640.0;
    const CHROME: f32 = 96.0; // room for the two toolbars
    let scale = (MAX / w.max(1.0)).min(MAX / h.max(1.0)).min(1.0);
    [
        (w * scale).clamp(MIN, MAX),
        (h * scale + CHROME).clamp(MIN, MAX),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every icon the packaging installs must exist and be a real PNG.
    ///
    /// These are generated by `assets/generate-icons.py` and committed, so the
    /// build needs no image tooling — which also means nothing regenerates them
    /// if the logo changes and someone forgets. A missing size is invisible
    /// until a desktop picks exactly that one and shows a broken image.
    #[test]
    fn every_shipped_icon_size_exists_and_decodes() {
        const SIZES: [u32; 8] = [16, 24, 32, 48, 64, 128, 256, 512];
        for size in SIZES {
            let path = format!(
                "{}/../../assets/icons/bettershot-{size}.png",
                env!("CARGO_MANIFEST_DIR")
            );
            let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
            assert_eq!(
                &bytes[..8],
                b"\x89PNG\r\n\x1a\n",
                "{path} is not a PNG; regenerate with assets/generate-icons.py"
            );
            let decoded = image::load_from_memory(&bytes)
                .unwrap_or_else(|e| panic!("{path} will not decode: {e}"));
            assert_eq!(
                (decoded.width(), decoded.height()),
                (size, size),
                "{path} is the wrong size"
            );
        }
    }

    /// The two icons compiled into the binary are the ones that must never be
    /// missing: `include_bytes!` would fail the build, but a *corrupt* file
    /// would only fail at runtime, costing the tray icon or the window icon.
    #[test]
    fn the_embedded_icons_decode() {
        for (name, bytes) in [
            (
                "tray",
                &include_bytes!("../../../assets/icons/bettershot-32.png")[..],
            ),
            (
                "window",
                &include_bytes!("../../../assets/icons/bettershot-256.png")[..],
            ),
        ] {
            let decoded = image::load_from_memory(bytes)
                .unwrap_or_else(|e| panic!("the {name} icon will not decode: {e}"));
            assert!(decoded.width() > 0 && decoded.height() > 0);
        }
    }

    #[test]
    fn a_huge_screenshot_opens_in_a_reasonably_sized_window() {
        let [w, h] = initial_window_size([3840.0, 2160.0]);
        assert!(w <= 1600.0 && h <= 1600.0, "{w}x{h}");
        assert!(w >= 640.0 && h >= 640.0);
        // Aspect ratio is roughly preserved, ignoring the toolbar allowance.
        assert!((w / (h - 96.0) - 3840.0 / 2160.0).abs() < 0.1, "{w}x{h}");
    }

    #[test]
    fn a_tiny_screenshot_still_gets_a_usable_window() {
        let [w, h] = initial_window_size([20.0, 20.0]);
        assert!(w >= 640.0 && h >= 640.0, "{w}x{h}");
    }

    #[test]
    fn a_zero_sized_image_does_not_produce_nan() {
        let [w, h] = initial_window_size([0.0, 0.0]);
        assert!(w.is_finite() && h.is_finite(), "{w}x{h}");
    }
}
