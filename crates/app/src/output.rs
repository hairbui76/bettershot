//! Saving and copying the annotated image.
//!
//! Everything here works on a freshly rendered copy of the document rather
//! than on whatever the editor happens to have on screen: the exported image
//! must be at full resolution and unaffected by zoom, pan or the toolbars.
//! That is why export goes through the CPU rasterizer in `bettershot-render`
//! rather than reading back the GPU surface.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use bettershot_core::Scene;
use bettershot_core::config::{Config, SaveFormat};
use image::RgbaImage;

/// The clipboard handle is kept alive for the life of the process.
///
/// On X11 there is no clipboard *server*: the owning application holds the
/// selection and hands it over on request. arboard tears that down when the
/// last handle drops — it offers the data to a clipboard manager, destroys the
/// owner window and joins its thread — so creating a handle per copy meant the
/// image could vanish the instant the function returned, on any session
/// without a clipboard manager (a bare WM, i3, sway-on-X11). bettershot
/// reported success and fired a notification regardless.
static CLIPBOARD: std::sync::Mutex<Option<arboard::Clipboard>> = std::sync::Mutex::new(None);

/// Run `f` against the long-lived clipboard handle.
fn with_clipboard<T>(
    f: impl FnOnce(&mut arboard::Clipboard) -> Result<T, arboard::Error>,
) -> Result<T, OutputError> {
    let mut guard = CLIPBOARD
        .lock()
        .map_err(|_| OutputError::Clipboard("the clipboard handle is poisoned".into()))?;
    if guard.is_none() {
        *guard =
            Some(arboard::Clipboard::new().map_err(|e| OutputError::Clipboard(e.to_string()))?);
    }
    let clipboard = guard.as_mut().expect("just populated");
    f(clipboard).map_err(|e| OutputError::Clipboard(e.to_string()))
}

#[derive(Debug, thiserror::Error)]
pub enum OutputError {
    #[error("no output filename configured; use --output-filename or Ctrl+Shift+S")]
    NoOutputPath,
    #[error("could not encode the image: {0}")]
    Encode(String),
    #[error("could not write {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("clipboard unavailable: {0}")]
    Clipboard(String),
    #[error("copy command `{command}` failed: {message}")]
    CopyCommand { command: String, message: String },
}

/// Render the document at full resolution.
///
/// This is the single point where the annotated result is produced, so the
/// clipboard and every file format see byte-identical pixels. It goes through
/// the CPU rasterizer rather than the on-screen renderer, so the output is
/// unaffected by zoom, pan, or the size of the window.
pub fn render_annotated(base: &RgbaImage, scene: &Scene) -> RgbaImage {
    let width = base.width();
    let height = base.height();
    let Ok(canvas) = bettershot_render::Canvas::from_rgba8(width, height, base.as_raw().clone())
    else {
        // Only possible if the buffer length disagreed with the dimensions,
        // which `RgbaImage` guarantees it does not.
        log::error!("could not build a render canvas for a {width}×{height} image");
        return base.clone();
    };

    let rendered = bettershot_render::render_scene(&canvas, scene);
    let (out_w, out_h) = (rendered.width(), rendered.height());
    RgbaImage::from_raw(out_w, out_h, rendered.into_rgba8()).unwrap_or_else(|| {
        log::error!("the renderer returned a malformed {out_w}×{out_h} buffer");
        base.clone()
    })
}

/// Encode to the configured format.
pub fn encode(image: &RgbaImage, format: SaveFormat) -> Result<Vec<u8>, OutputError> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    let encoded = match format {
        SaveFormat::Png => image.write_to(&mut buffer, image::ImageFormat::Png),
        SaveFormat::Webp => image.write_to(&mut buffer, image::ImageFormat::WebP),
        SaveFormat::Jpeg => {
            // JPEG has no alpha channel, so flatten onto white rather than
            // letting the encoder reject the image or produce black fringes.
            let flattened = flatten_on_white(image);
            flattened.write_to(&mut buffer, image::ImageFormat::Jpeg)
        }
    };
    encoded.map_err(|e| OutputError::Encode(e.to_string()))?;
    Ok(buffer.into_inner())
}

fn flatten_on_white(image: &RgbaImage) -> image::RgbImage {
    image::RgbImage::from_fn(image.width(), image.height(), |x, y| {
        let [r, g, b, a] = image.get_pixel(x, y).0;
        let blend = |c: u8| {
            let a = a as f32 / 255.0;
            (c as f32 * a + 255.0 * (1.0 - a)).round() as u8
        };
        image::Rgb([blend(r), blend(g), blend(b)])
    })
}

/// Write the annotated image to `config.output_filename`.
pub fn save_to_file(
    base: &RgbaImage,
    scene: &Scene,
    config: &Config,
) -> Result<PathBuf, OutputError> {
    // Resolved at save time, not at startup, so a %H-%M-%S template records
    // when the image was written.
    let path = bettershot_cli::resolve_output_path(config, chrono::Local::now())
        .ok_or(OutputError::NoOutputPath)?;
    write_image(base, scene, config, &path)?;
    Ok(path)
}

/// Ask the user where to save, then write there.
pub fn save_with_dialog(
    base: &RgbaImage,
    scene: &Scene,
    config: &Config,
) -> Result<Option<PathBuf>, OutputError> {
    let mut dialog = rfd::FileDialog::new().add_filter(
        config.save_format.extension(),
        &[config.save_format.extension()],
    );
    // Start where the configured output would have gone, so Save-As is a
    // refinement of Save rather than a fresh start.
    if let Some(suggested) = bettershot_cli::resolve_output_path(config, chrono::Local::now()) {
        if let Some(parent) = suggested.parent().filter(|p| p.is_dir()) {
            dialog = dialog.set_directory(parent);
        }
        if let Some(name) = suggested.file_name().and_then(|n| n.to_str()) {
            dialog = dialog.set_file_name(name);
        }
    }

    let Some(path) = dialog.save_file() else {
        return Ok(None);
    };
    write_image(base, scene, config, &path)?;
    Ok(Some(path))
}

/// `--output-filename -` means stdout, matching the `-` that `--filename`
/// already accepts for stdin. The CLI deliberately refuses to give this an
/// extension; without a branch here it created a file literally named `-` in
/// whatever directory the process happened to start in — for a compositor
/// keybinding, typically `/` or the home directory.
const STDOUT: &str = "-";

fn write_image(
    base: &RgbaImage,
    scene: &Scene,
    config: &Config,
    path: &Path,
) -> Result<(), OutputError> {
    if path.as_os_str() == STDOUT {
        let bytes = encode(&render_annotated(base, scene), config.save_format)?;
        let mut out = std::io::stdout().lock();
        out.write_all(&bytes)
            .and_then(|()| out.flush())
            .map_err(|source| OutputError::Write {
                path: PathBuf::from(STDOUT),
                source,
            })?;
        return Ok(());
    }

    // Honour the extension the user actually chose, not just the configured
    // default: saving as `shot.jpg` should produce a JPEG.
    let format = path
        .to_str()
        .map(SaveFormat::from_path)
        .unwrap_or(config.save_format);
    let bytes = encode(&render_annotated(base, scene), format)?;

    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|source| OutputError::Write {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    std::fs::write(path, bytes).map_err(|source| OutputError::Write {
        path: path.to_path_buf(),
        source,
    })
}

/// Put the annotated image on the system clipboard.
///
/// A configured `copy-command` wins, because on some Wayland setups no
/// in-process clipboard API works and piping to `wl-copy` is the only reliable
/// route.
pub fn copy_to_clipboard(
    base: &RgbaImage,
    scene: &Scene,
    config: &Config,
) -> Result<(), OutputError> {
    let rendered = render_annotated(base, scene);

    if let Some(command) = &config.copy_command {
        let png = encode(&rendered, SaveFormat::Png)?;
        return pipe_to_command(command, &png);
    }

    with_clipboard(|clipboard| {
        clipboard.set_image(arboard::ImageData {
            width: rendered.width() as usize,
            height: rendered.height() as usize,
            bytes: std::borrow::Cow::Borrowed(rendered.as_raw()),
        })
    })
}

/// Put an already-encoded PNG on the clipboard, for re-copying from history.
pub fn copy_png_bytes(png: &[u8], config: &Config) -> Result<(), OutputError> {
    if let Some(command) = &config.copy_command {
        return pipe_to_command(command, png);
    }
    let decoded = image::load_from_memory(png)
        .map_err(|e| OutputError::Encode(e.to_string()))?
        .to_rgba8();
    with_clipboard(|clipboard| {
        clipboard.set_image(arboard::ImageData {
            width: decoded.width() as usize,
            height: decoded.height() as usize,
            bytes: std::borrow::Cow::Borrowed(decoded.as_raw()),
        })
    })
}

/// Put plain text on the clipboard, for the copy-path action.
pub fn copy_text(text: &str) -> Result<(), OutputError> {
    with_clipboard(|clipboard| clipboard.set_text(text.to_owned()))
}

/// Run `command` through the shell with the image on its stdin.
fn pipe_to_command(command: &str, data: &[u8]) -> Result<(), OutputError> {
    let fail = |message: String| OutputError::CopyCommand {
        command: command.to_owned(),
        message,
    };

    let mut child = shell_command(command)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .map_err(|e| fail(e.to_string()))?;

    child
        .stdin
        .as_mut()
        .ok_or_else(|| fail("could not open stdin".into()))?
        .write_all(data)
        .map_err(|e| fail(e.to_string()))?;

    let status = child.wait().map_err(|e| fail(e.to_string()))?;
    if status.success() {
        Ok(())
    } else {
        Err(fail(format!("exited with {status}")))
    }
}

fn shell_command(command: &str) -> Command {
    if cfg!(target_os = "windows") {
        let mut c = Command::new("cmd");
        c.args(["/C", command]);
        c
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", command]);
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bettershot_core::math::{Rect, Vec2D};
    use bettershot_core::style::Style;
    use bettershot_core::tools::Rectangle;

    fn base() -> RgbaImage {
        RgbaImage::from_pixel(40, 30, image::Rgba([255, 255, 255, 255]))
    }

    fn scene_with_a_rectangle() -> Scene {
        let mut scene = Scene::new(Vec2D::new(40.0, 30.0));
        scene.add(Box::new(Rectangle {
            rect: Rect::from_xywh(5.0, 5.0, 20.0, 15.0),
            style: Style::default().with_fill(true),
        }));
        scene
    }

    #[test]
    fn rendering_keeps_the_full_image_size() {
        let rendered = render_annotated(&base(), &scene_with_a_rectangle());
        assert_eq!(rendered.dimensions(), (40, 30));
    }

    #[test]
    fn rendering_actually_draws_the_annotation() {
        let rendered = render_annotated(&base(), &scene_with_a_rectangle());
        // Inside the rectangle the white base must have been painted over.
        assert_ne!(
            rendered.get_pixel(15, 12).0,
            [255, 255, 255, 255],
            "the annotation was not rendered"
        );
        // A corner well outside it must be untouched.
        assert_eq!(rendered.get_pixel(38, 28).0, [255, 255, 255, 255]);
    }

    #[test]
    fn png_and_webp_round_trip_through_the_encoder() {
        let rendered = render_annotated(&base(), &scene_with_a_rectangle());
        for format in [SaveFormat::Png, SaveFormat::Webp] {
            let bytes = encode(&rendered, format).expect("should encode");
            assert!(!bytes.is_empty(), "{format:?} produced nothing");
            let decoded = image::load_from_memory(&bytes).expect("should decode");
            assert_eq!(decoded.width(), 40);
            assert_eq!(decoded.height(), 30);
        }
    }

    #[test]
    fn jpeg_export_flattens_transparency_instead_of_failing() {
        let mut transparent = base();
        transparent.put_pixel(0, 0, image::Rgba([0, 0, 0, 0]));
        let bytes = encode(&transparent, SaveFormat::Jpeg).expect("JPEG should encode");
        let decoded = image::load_from_memory(&bytes).expect("should decode");
        assert_eq!(decoded.width(), 40);
    }

    #[test]
    fn flattening_composites_alpha_onto_white() {
        let mut img = RgbaImage::new(1, 1);
        img.put_pixel(0, 0, image::Rgba([0, 0, 0, 128]));
        let flat = flatten_on_white(&img);
        let v = flat.get_pixel(0, 0).0[0];
        assert!((126..=129).contains(&v), "expected mid grey, got {v}");
    }

    #[test]
    fn saving_without_a_configured_path_is_a_clear_error() {
        let config = Config::default();
        let err = save_to_file(&base(), &scene_with_a_rectangle(), &config).unwrap_err();
        assert!(matches!(err, OutputError::NoOutputPath));
    }

    #[test]
    fn a_dash_output_path_writes_to_stdout_not_a_file_called_dash() {
        // The CLI documents and tests `-` as meaning stdout; without a branch
        // in the writer it silently created a file named `-` in the process
        // working directory.
        let dir = tempfile::tempdir().expect("tempdir");
        let previous = std::env::current_dir().expect("cwd");
        std::env::set_current_dir(dir.path()).expect("chdir");

        let config = Config::default();
        let result = write_image(
            &base(),
            &scene_with_a_rectangle(),
            &config,
            std::path::Path::new("-"),
        );

        let stray = dir.path().join("-");
        let existed = stray.exists();
        std::env::set_current_dir(previous).expect("restore cwd");

        assert!(
            result.is_ok(),
            "writing to stdout should succeed: {result:?}"
        );
        assert!(
            !existed,
            "a file named `-` was created instead of using stdout"
        );
    }

    #[test]
    fn saving_creates_missing_parent_directories() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested/deeper/shot.png");
        let config = Config::default();
        write_image(&base(), &scene_with_a_rectangle(), &config, &path).expect("should save");
        assert!(path.exists());
        assert!(image::open(&path).is_ok(), "wrote a decodable image");
    }

    #[test]
    fn the_extension_chooses_the_format_over_the_configured_default() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("shot.jpg");
        let config = Config::default(); // save_format defaults to PNG
        write_image(&base(), &scene_with_a_rectangle(), &config, &path).expect("should save");

        let decoded = image::ImageReader::open(&path)
            .expect("open")
            .with_guessed_format()
            .expect("format");
        assert_eq!(decoded.format(), Some(image::ImageFormat::Jpeg));
    }

    /// Unix only, and deliberately so: this needs a shell command that copies
    /// stdin to a file byte for byte, and `cmd.exe` has no reliable one —
    /// `more` is a text pager that mangles binary. The test previously used
    /// `cat`, which exists on a Windows runner only because Git bundles it, so
    /// it passed or failed depending on PATH. `copy-command` is a workaround
    /// for Wayland sessions where no in-process clipboard API works; Windows
    /// has a working clipboard and does not need it.
    ///
    /// The cross-platform half — that a failing command is reported rather
    /// than swallowed — is covered below and runs everywhere.
    #[cfg(unix)]
    #[test]
    fn a_copy_command_receives_the_png_on_stdin() {
        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("piped.png");
        let config = Config {
            copy_command: Some(format!("cat > {}", target.display())),
            ..Default::default()
        };

        copy_to_clipboard(&base(), &scene_with_a_rectangle(), &config)
            .expect("the copy command should succeed");

        let written = std::fs::read(&target).expect("the command should have written the file");
        assert!(!written.is_empty());
        assert_eq!(&written[1..4], b"PNG", "stdin received a PNG");
    }

    #[test]
    fn a_failing_copy_command_is_reported_not_swallowed() {
        let config = Config {
            copy_command: Some("exit 3".into()),
            ..Default::default()
        };
        let err = copy_to_clipboard(&base(), &scene_with_a_rectangle(), &config).unwrap_err();
        assert!(
            matches!(err, OutputError::CopyCommand { .. }),
            "got {err:?}"
        );
    }
}
