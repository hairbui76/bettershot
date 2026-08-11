//! Linux / X11: a direct grab through the X protocol.
//!
//! Implemented on `x11rb`'s pure-Rust connection rather than `xcap`. `xcap` on
//! Linux drags in `libwayshot`/`gbm`/`wayland-client`, which need Wayland and
//! DRM *development* libraries present at build time even for an X11-only
//! build; that breaks headless build machines. `x11rb` speaks the wire protocol
//! itself and needs no system libraries at all.
//!
//! Caveats specific to X11:
//!
//! * **No per-monitor DPI.** X11 has one global pixel grid, so every monitor is
//!   reported with `scale_factor: 1.0`. HiDPI on X11 is done by the toolkit
//!   (`GDK_SCALE`, `Xft.dpi`), not the server, and is the app shell's business.
//! * **Monitor ids are enumeration indices**, valid until the display
//!   configuration changes — like every other backend's ids.
//! * **Window geometry excludes decorations** drawn by a reparenting window
//!   manager, because it comes from the client window's own geometry translated
//!   to the root.
//! * **`GetImage` reads the root window**, so occluded parts of a window are
//!   whatever is painted on top of them. Capturing a background window is not
//!   possible without the Composite extension.
//! * Grabs are subject to no permission prompt at all: any X client can read
//!   the whole screen. That is an X11 design decision, not a bettershot one.

use bettershot_core::{Rect, Vec2D};
use x11rb::connection::Connection;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::xfixes;
use x11rb::protocol::xproto::{self, AtomEnum, ConnectionExt as _, ImageFormat, ImageOrder};
use x11rb::rust_connection::RustConnection;

use crate::{
    Capabilities, CaptureBackend, CaptureError, CaptureTarget, CursorAnchor, CursorImage,
    MonitorId, MonitorInfo, RawFrame, VirtualDesktop, WindowId, WindowInfo,
    geometry::PixelRect,
    pixels::{ZPixmapFormat, zpixmap_to_rgba},
    target::resolve_target,
};

/// Interned atom ids this backend needs.
struct Atoms {
    net_client_list_stacking: xproto::Atom,
    net_wm_name: xproto::Atom,
    net_wm_state: xproto::Atom,
    net_wm_state_hidden: xproto::Atom,
    /// A *type* atom, never a property name: it is the `type` argument of
    /// `GetProperty` for `_NET_WM_NAME`, and the value [`property_text`] tests
    /// a reply's type against to decide between UTF-8 and Latin-1.
    ///
    /// [`property_text`]: X11Backend::property_text
    utf8_string: xproto::Atom,
    wm_class: xproto::Atom,
}

/// Direct X11 capture backend.
pub(crate) struct X11Backend {
    conn: RustConnection,
    root: xproto::Window,
    root_size: (u16, u16),
    format: ZPixmapFormat,
    atoms: Atoms,
    /// Whether XFixes is present, negotiated once at connect time. The cursor
    /// is the only thing this backend uses it for, and plenty of minimal or
    /// remote X servers ship without it.
    has_xfixes: bool,
}

impl X11Backend {
    /// Connect to the display named by `DISPLAY`.
    pub(crate) fn connect() -> Result<Self, CaptureError> {
        let (conn, screen_num) = x11rb::connect(None).map_err(|err| {
            CaptureError::backend(format!("could not connect to the X server: {err}"))
        })?;

        let (root, root_size, format) = {
            let setup = conn.setup();
            let screen = setup.roots.get(screen_num).ok_or_else(|| {
                CaptureError::backend(format!("the X server has no screen {screen_num}"))
            })?;

            let visual = screen
                .allowed_depths
                .iter()
                .flat_map(|depth| depth.visuals.iter())
                .find(|v| v.visual_id == screen.root_visual)
                .ok_or_else(|| {
                    CaptureError::backend("the X server did not describe the root visual")
                })?;
            let pixmap_format = setup
                .pixmap_formats
                .iter()
                .find(|f| f.depth == screen.root_depth)
                .ok_or_else(|| {
                    CaptureError::backend(format!(
                        "the X server has no pixmap format for depth {}",
                        screen.root_depth
                    ))
                })?;

            (
                screen.root,
                (screen.width_in_pixels, screen.height_in_pixels),
                ZPixmapFormat {
                    bits_per_pixel: pixmap_format.bits_per_pixel,
                    scanline_pad: pixmap_format.scanline_pad,
                    little_endian: setup.image_byte_order == ImageOrder::LSB_FIRST,
                    red_mask: visual.red_mask,
                    green_mask: visual.green_mask,
                    blue_mask: visual.blue_mask,
                },
            )
        };

        let atoms = Atoms {
            net_client_list_stacking: intern(&conn, b"_NET_CLIENT_LIST_STACKING")?,
            net_wm_name: intern(&conn, b"_NET_WM_NAME")?,
            net_wm_state: intern(&conn, b"_NET_WM_STATE")?,
            net_wm_state_hidden: intern(&conn, b"_NET_WM_STATE_HIDDEN")?,
            utf8_string: intern(&conn, b"UTF8_STRING")?,
            wm_class: intern(&conn, b"WM_CLASS")?,
        };

        // XFixes requires a version handshake before any of its other requests,
        // and answers with an error when the extension is not present at all.
        // Either way the answer is just "no cursor from this server", so the
        // failure is recorded rather than propagated: it must not stop a
        // screenshot.
        // Version 1 is deliberate: `GetCursorImage` is a version-1 request, and
        // asking for the lowest version that has it keeps the handshake working
        // against the oldest servers.
        let has_xfixes = xfixes::query_version(&conn, 1, 0)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .is_some();
        if !has_xfixes {
            log::debug!("the X server has no XFixes extension; --include-cursor will be inert");
        }

        Ok(Self {
            conn,
            root,
            root_size,
            format,
            atoms,
            has_xfixes,
        })
    }

    /// RandR 1.5 monitors, falling back to "the whole screen is one monitor"
    /// when RandR is missing or too old (Xvfb, very old servers, some remote X
    /// implementations).
    fn randr_monitors(&self) -> Result<Vec<MonitorInfo>, CaptureError> {
        self.conn
            .randr_query_version(1, 5)
            .map_err(backend_err)?
            .reply()
            .map_err(backend_err)?;
        let reply = self
            .conn
            .randr_get_monitors(self.root, true)
            .map_err(backend_err)?
            .reply()
            .map_err(backend_err)?;

        Ok(reply
            .monitors
            .iter()
            .enumerate()
            .map(|(index, m)| {
                let name = self
                    .atom_name(m.name)
                    .unwrap_or_else(|| format!("output-{index}"));
                MonitorInfo::new(
                    MonitorId::new(index as u64),
                    name,
                    Rect::from_xywh(
                        f32::from(m.x),
                        f32::from(m.y),
                        f32::from(m.width),
                        f32::from(m.height),
                    ),
                    // X11 has no per-monitor scale; see the module docs.
                    1.0,
                    m.primary,
                )
            })
            .collect())
    }

    fn whole_screen_monitor(&self) -> MonitorInfo {
        MonitorInfo::new(
            MonitorId::new(0),
            "screen",
            Rect::from_xywh(
                0.0,
                0.0,
                f32::from(self.root_size.0),
                f32::from(self.root_size.1),
            ),
            1.0,
            true,
        )
    }

    fn atom_name(&self, atom: xproto::Atom) -> Option<String> {
        let reply = self.conn.get_atom_name(atom).ok()?.reply().ok()?;
        String::from_utf8(reply.name).ok()
    }

    /// Read a property as a list of 32-bit values (atoms, window ids, ...).
    fn property_u32(&self, window: xproto::Window, property: xproto::Atom) -> Vec<u32> {
        let Ok(cookie) =
            self.conn
                .get_property(false, window, property, AtomEnum::ANY, 0, u32::MAX / 4)
        else {
            return Vec::new();
        };
        cookie
            .reply()
            .ok()
            .and_then(|reply| reply.value32().map(|iter| iter.collect()))
            .unwrap_or_default()
    }

    /// Read a text property and decode it according to the type the server
    /// reports back.
    ///
    /// `wanted_type` is the `type` argument of `GetProperty`: name the type the
    /// specification mandates and the server filters for you, or
    /// [`AtomEnum::ANY`] to take whatever is there. It is *not* a property
    /// name — asking for a property called `UTF8_STRING` matches nothing,
    /// because `UTF8_STRING` only ever appears as a type.
    fn property_text(
        &self,
        window: xproto::Window,
        property: xproto::Atom,
        wanted_type: xproto::Atom,
    ) -> Option<String> {
        let reply = self
            .conn
            .get_property(false, window, property, wanted_type, 0, 4096)
            .ok()?
            .reply()
            .ok()?;
        if reply.value.is_empty() {
            return None;
        }
        Some(decode_text(
            &reply.value,
            reply.type_ == self.atoms.utf8_string,
        ))
    }

    fn window_title(&self, window: xproto::Window) -> String {
        // `_NET_WM_NAME` is UTF-8 and authoritative: EWMH mandates the type,
        // so it is requested by type and the server rejects anything else.
        //
        // `WM_NAME` is the ICCCM fallback, whose type is `TEXT` — in practice
        // `STRING` (Latin-1) from old clients, but `UTF8_STRING` or
        // `COMPOUND_TEXT` from some toolkits. Asking for `ANY` and letting
        // `decode_text` follow the reply's own type is what keeps an accented
        // legacy title from coming back as a row of U+FFFD.
        self.property_text(window, self.atoms.net_wm_name, self.atoms.utf8_string)
            .or_else(|| self.property_text(window, AtomEnum::WM_NAME.into(), AtomEnum::ANY.into()))
            .map(|s| s.trim_end_matches('\0').to_owned())
            .unwrap_or_default()
    }

    /// `WM_CLASS` is two NUL-terminated strings: instance then class. The class
    /// is the better application name.
    ///
    /// ICCCM types it `STRING`, i.e. Latin-1, but requesting `ANY` and decoding
    /// by the reply's type also picks up the clients that write it as UTF-8.
    fn window_app_name(&self, window: xproto::Window) -> String {
        let raw = self
            .property_text(window, self.atoms.wm_class, AtomEnum::ANY.into())
            .unwrap_or_default();
        let mut parts = raw.split('\0').filter(|s| !s.is_empty());
        let instance = parts.next().unwrap_or_default().to_owned();
        parts.next().map(str::to_owned).unwrap_or(instance)
    }

    fn window_is_minimized(&self, window: xproto::Window) -> bool {
        self.property_u32(window, self.atoms.net_wm_state)
            .contains(&self.atoms.net_wm_state_hidden)
    }

    /// Frame position on the root window, in physical pixels.
    fn window_bounds(&self, window: xproto::Window) -> Option<Rect> {
        let geometry = self.conn.get_geometry(window).ok()?.reply().ok()?;
        let translated = self
            .conn
            .translate_coordinates(window, self.root, 0, 0)
            .ok()?
            .reply()
            .ok()?;
        Some(Rect::from_xywh(
            f32::from(translated.dst_x),
            f32::from(translated.dst_y),
            f32::from(geometry.width),
            f32::from(geometry.height),
        ))
    }

    /// One `GetImage` against the root window.
    fn grab(&self, rect: PixelRect, scale_factor: f32) -> Result<RawFrame, CaptureError> {
        let x = i16::try_from(rect.x)
            .map_err(|_| CaptureError::invalid_frame("region x does not fit in an X11 i16"))?;
        let y = i16::try_from(rect.y)
            .map_err(|_| CaptureError::invalid_frame("region y does not fit in an X11 i16"))?;
        let width = u16::try_from(rect.width)
            .map_err(|_| CaptureError::invalid_frame("region is wider than 65535 px"))?;
        let height = u16::try_from(rect.height)
            .map_err(|_| CaptureError::invalid_frame("region is taller than 65535 px"))?;

        let reply = self
            .conn
            .get_image(
                ImageFormat::Z_PIXMAP,
                self.root,
                x,
                y,
                width,
                height,
                u32::MAX,
            )
            .map_err(backend_err)?
            .reply()
            .map_err(backend_err)?;

        let data = zpixmap_to_rgba(&reply.data, rect.width, rect.height, self.format)?;
        RawFrame::new(
            rect.width,
            rect.height,
            data,
            Vec2D::new(rect.x as f32, rect.y as f32),
            scale_factor,
        )
    }
}

impl CaptureBackend for X11Backend {
    fn name(&self) -> &'static str {
        "x11"
    }

    fn monitors(&self) -> Result<Vec<MonitorInfo>, CaptureError> {
        match self.randr_monitors() {
            Ok(monitors) if !monitors.is_empty() => Ok(monitors),
            Ok(_) => {
                log::debug!("RandR reported no monitors; falling back to the root window");
                Ok(vec![self.whole_screen_monitor()])
            }
            Err(err) => {
                log::debug!("RandR monitor query failed ({err}); using the root window");
                Ok(vec![self.whole_screen_monitor()])
            }
        }
    }

    fn windows(&self) -> Result<Vec<WindowInfo>, CaptureError> {
        // `_NET_CLIENT_LIST_STACKING` is bottom-to-top; bettershot wants
        // 0 = frontmost, so the index is inverted.
        let stacking = self.property_u32(self.root, self.atoms.net_client_list_stacking);
        let count = stacking.len();
        let mut out = Vec::with_capacity(count);
        for (index, &window) in stacking.iter().enumerate() {
            let Some(bounds) = self.window_bounds(window) else {
                // Raced with the window closing; skip it rather than fail the
                // whole enumeration.
                continue;
            };
            out.push(WindowInfo::new(
                WindowId::new(u64::from(window)),
                self.window_title(window),
                self.window_app_name(window),
                bounds,
                self.window_is_minimized(window),
                (count - 1 - index) as u32,
            ));
        }
        Ok(out)
    }

    fn capture(&self, target: CaptureTarget) -> Result<RawFrame, CaptureError> {
        let desktop = VirtualDesktop::new(self.monitors()?);
        let windows = match target {
            CaptureTarget::Window(_) => self.windows()?,
            _ => Vec::new(),
        };
        let resolved = resolve_target(target, &desktop, &windows)?;
        // The X11 root window spans the whole virtual screen, so even a
        // full-desktop capture is a single grab — no stitching needed.
        self.grab(resolved.pixel_bounds()?, resolved.scale_factor)
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            cursor: self.has_xfixes,
            ..Capabilities::FULL
        }
    }

    fn cursor(&self) -> Result<Option<CursorImage>, CaptureError> {
        if !self.has_xfixes {
            return Ok(None);
        }
        let reply = xfixes::get_cursor_image(&self.conn)
            .map_err(backend_err)?
            .reply()
            .map_err(backend_err)?;

        cursor_from_xfixes(
            reply.x,
            reply.y,
            reply.width,
            reply.height,
            reply.xhot,
            reply.yhot,
            &reply.cursor_image,
        )
    }
}

/// Turn an XFixes `GetCursorImage` reply into a [`CursorImage`].
///
/// Split out from the request so the interpretation of the reply — which is the
/// part that can be silently wrong — is testable without an X server.
///
/// The key decision is the anchor. `x`/`y` are the *sprite position*, i.e. the
/// hotspot, in root-window coordinates: `ProcXFixesGetCursorImage` assigns
/// `GetSpritePosition`'s result straight through and reports `xhot`/`yhot`
/// alongside it. The protocol spec's "x and y are the current cursor position"
/// reads as though it might already be the bitmap's corner; it is not, and
/// treating it as one draws the pointer offset by its hotspot.
///
/// Root-window coordinates are X11's virtual-desktop coordinates, which is the
/// space a [`RawFrame`]'s origin lives in, so no further rebasing is needed.
#[allow(clippy::too_many_arguments)]
fn cursor_from_xfixes(
    x: i16,
    y: i16,
    width: u16,
    height: u16,
    xhot: u16,
    yhot: u16,
    words: &[u32],
) -> Result<Option<CursorImage>, CaptureError> {
    // A hidden pointer comes back as a zero-sized image rather than an error.
    if width == 0 || height == 0 {
        return Ok(None);
    }
    CursorImage::from_argb_words(
        u32::from(width),
        u32::from(height),
        words,
        CursorAnchor::Hotspot {
            position: Vec2D::new(f32::from(x), f32::from(y)),
            xhot: u32::from(xhot),
            yhot: u32::from(yhot),
        },
    )
    .map(Some)
}

/// Decode an X11 text property's bytes.
///
/// X11 has two text encodings in everyday use and no way to guess between them
/// from the bytes alone, so the property's *type* decides:
///
/// * `UTF8_STRING` (`utf8` here) is UTF-8; malformed sequences become U+FFFD
///   rather than failing the whole enumeration over one bad title.
/// * Everything else — `STRING`, and `COMPOUND_TEXT`'s Latin-1 base set — is
///   ISO 8859-1, where every byte is the code point of the same number. Running
///   those bytes through a UTF-8 decoder instead is what turns `Café` into
///   `Caf<U+FFFD>`, because `é` is the lone byte `0xE9` and no UTF-8 sequence
///   starts that way.
///
/// Non-Latin-1 `COMPOUND_TEXT` (with ISO 2022 escape sequences) is not
/// decoded; it is vanishingly rare, and its ASCII-range bytes still survive.
fn decode_text(bytes: &[u8], utf8: bool) -> String {
    if utf8 {
        String::from_utf8_lossy(bytes).into_owned()
    } else {
        bytes.iter().map(|&b| char::from(b)).collect()
    }
}

fn intern(conn: &RustConnection, name: &[u8]) -> Result<xproto::Atom, CaptureError> {
    Ok(conn
        .intern_atom(false, name)
        .map_err(backend_err)?
        .reply()
        .map_err(backend_err)?
        .atom)
}

fn backend_err(err: impl std::fmt::Display) -> CaptureError {
    CaptureError::backend(format!("X11: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reply for a 2x2 cursor whose hotspot is one pixel in from the
    /// top-left, sitting at root coordinate (50, 60).
    fn sample_reply() -> Result<Option<CursorImage>, CaptureError> {
        cursor_from_xfixes(50, 60, 2, 2, 1, 1, &[0xffff_0000; 4])
    }

    #[test]
    fn the_reply_position_is_the_hotspot_not_the_bitmap_corner() {
        // The server reports where the *point of the arrow* is, so the bitmap
        // starts one pixel up and to the left of it. Reading the reply as a
        // corner would put the cursor at (50, 60) and draw it low and right.
        let cursor = sample_reply().unwrap().unwrap();
        assert_eq!(cursor.position, Vec2D::new(49.0, 59.0));
    }

    #[test]
    fn a_cursor_drawn_from_a_reply_lands_on_its_hotspot() {
        // End to end: the pixel the user thinks they are pointing at is the
        // one the hotspot covers.
        let cursor = sample_reply().unwrap().unwrap();
        let mut frame = RawFrame::filled(100, 100, [0, 0, 0, 255], Vec2D::ZERO, 1.0).unwrap();
        crate::composite_cursor(&mut frame, &cursor);
        assert_eq!(frame.pixel(50, 60), Some([255, 0, 0, 255]), "the hotspot");
        assert_eq!(frame.pixel(49, 59), Some([255, 0, 0, 255]), "top-left");
        // The bitmap is 2x2 anchored at (49, 59), so (51, 61) is past it.
        assert_eq!(frame.pixel(51, 61), Some([0, 0, 0, 255]));
    }

    #[test]
    fn a_hidden_cursor_is_reported_as_a_zero_sized_image() {
        assert!(cursor_from_xfixes(0, 0, 0, 0, 0, 0, &[]).unwrap().is_none());
    }

    #[test]
    fn a_negative_cursor_position_is_kept() {
        // Monitors left of or above the primary give the pointer negative root
        // coordinates; clamping them to zero would teleport the cursor.
        let cursor = cursor_from_xfixes(-30, -12, 1, 1, 0, 0, &[0xffff_ffff])
            .unwrap()
            .unwrap();
        assert_eq!(cursor.position, Vec2D::new(-30.0, -12.0));
    }

    #[test]
    fn a_truncated_cursor_reply_is_an_error_not_a_panic() {
        assert!(cursor_from_xfixes(0, 0, 8, 8, 0, 0, &[0; 2]).is_err());
    }

    #[test]
    fn utf8_titles_decode_as_utf8() {
        // "Café" as UTF-8: the e-acute is the two bytes C3 A9.
        let bytes = "Café — Firefox".as_bytes();
        assert_eq!(decode_text(bytes, true), "Café — Firefox");
    }

    #[test]
    fn legacy_latin1_titles_are_not_mangled_into_replacement_characters() {
        // The same title as a legacy `STRING` property: one byte per character,
        // and 0xE9 is not the start of any valid UTF-8 sequence.
        let bytes = [b'C', b'a', b'f', 0xE9];
        assert_eq!(decode_text(&bytes, false), "Café");
        // What the old unconditional `from_utf8_lossy` produced instead.
        assert_eq!(String::from_utf8_lossy(&bytes), "Caf\u{fffd}");
    }

    #[test]
    fn every_latin1_byte_maps_to_its_own_code_point() {
        let all: Vec<u8> = (0..=255).collect();
        let decoded = decode_text(&all, false);
        assert_eq!(decoded.chars().count(), 256);
        assert!(
            decoded
                .chars()
                .enumerate()
                .all(|(i, c)| c as u32 == i as u32)
        );
    }

    #[test]
    fn malformed_utf8_degrades_rather_than_failing_the_enumeration() {
        assert_eq!(decode_text(&[b'a', 0xFF, b'b'], true), "a\u{fffd}b");
    }

    #[test]
    fn ascii_decodes_identically_under_either_type() {
        let bytes = b"kitty";
        assert_eq!(decode_text(bytes, true), decode_text(bytes, false));
    }
}
