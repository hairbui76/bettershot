# Platform setup

bettershot captures the screen itself, and screen capture is the most
OS-specific thing a program can do. This page covers what each platform needs.

## Linux

### Which backend gets used

bettershot picks a backend from the session type, in this order:

| Session | Backend | Notes |
| --- | --- | --- |
| Wayland (`WAYLAND_DISPLAY` set) | xdg-desktop-portal | The compositor, not bettershot, takes the shot. A permission dialog may appear. |
| X11 (`DISPLAY` set, no Wayland) | Direct X11 grab | No permission prompt. |
| Neither | *fails with a clear error* | You are on a TTY or over plain SSH; there is nothing to capture. |

Run with `-v` to see which backend was chosen.

### Wayland

You need a working portal for your compositor:

| Compositor | Package |
| --- | --- |
| GNOME | `xdg-desktop-portal-gnome` |
| KDE Plasma | `xdg-desktop-portal-kde` |
| Sway, Hyprland, River, and other wlroots compositors | `xdg-desktop-portal-wlr` |

Plus the base `xdg-desktop-portal` package. If capture fails with a permission
error, check that the portal service is running:

```sh
systemctl --user status xdg-desktop-portal
```

**Compositor quirks.** Portal behaviour is not uniform:

- Some compositors show their own region picker before handing back an image.
  Where that happens, bettershot's own overlay is redundant; use
  `--capture monitor` and crop in the editor instead.
- `xdg-desktop-portal-wlr` needs to know which output to use and may prompt.
- Some portals return a URI to a temporary file rather than pixels directly.
  bettershot handles both.

### Global hotkeys on Wayland

Wayland deliberately does not let an application grab a global hotkey for
itself, so `bettershot --daemon` will report that hotkeys are unavailable and
keep running with only the tray (if built with `--features tray`). Bind your
compositor's key instead:

**Sway / i3** (`~/.config/sway/config`):
```
bindsym Print exec bettershot --capture region
bindsym Shift+Print exec bettershot --capture window
```

**Hyprland** (`~/.config/hypr/hyprland.conf`):
```
bind = , Print, exec, bettershot --capture region
bind = SHIFT, Print, exec, bettershot --capture window
```

**GNOME**: Settings → Keyboard → Custom Shortcuts.

**KDE**: System Settings → Shortcuts → Custom Shortcuts.

### Clipboard

The built-in clipboard works on X11 and on most Wayland compositors. If copying
silently does nothing on your setup, fall back to piping:

```toml
copy-command = "wl-copy --type image/png"
```

### Build dependencies

Only needed if you are building from source:

```sh
# Debian / Ubuntu
sudo apt install libxkbcommon-dev libwayland-dev libxcb1-dev \
    libxcb-randr0-dev libxcb-shm0-dev libxcb-xfixes0-dev libdbus-1-dev pkg-config

# Fedora
sudo dnf install libxkbcommon-devel wayland-devel libxcb-devel dbus-devel

# Arch
sudo pacman -S libxkbcommon wayland libxcb dbus
```

Building **with `--features tray`** needs three more, because the tray pulls in
GTK 3 and its menu layer links `libxdo`:

```sh
# Debian / Ubuntu
sudo apt install libgtk-3-dev libayatana-appindicator3-dev libxdo-dev

# Fedora
sudo dnf install gtk3-devel libayatana-appindicator-gtk3-devel libxdo-devel

# Arch
sudo pacman -S gtk3 libayatana-appindicator xdotool
```

## Windows

Windows 10 1903+ or Windows 11. No configuration is needed and no permission
prompt appears; capture uses Windows Graphics Capture, falling back to DXGI
desktop duplication on older builds.

### Mixed-DPI multi-monitor

bettershot keeps all capture geometry in **physical pixels** with each
monitor's scale factor recorded alongside it. A 150%-scaled laptop panel next
to a 100% external display captures at each display's true pixel size, and the
stitched full-desktop image preserves both. If you see a capture that is
half-size or offset on one monitor, that is a bug worth reporting — include the
output of `bettershot -vv --capture all`.

### Global hotkeys and the tray

Windows allows global hotkeys, so `bettershot --daemon` works fully here:

```
bettershot --daemon
```

registers `PrintScreen`, `Shift+PrintScreen` and `Ctrl+PrintScreen` by default
and shows a tray icon. If another application already owns a key, bettershot
says so and leaves the others working.

Official Windows builds are compiled with `--features tray`. If you build it
yourself, pass that flag or you will get hotkeys without a tray icon.

## macOS

**Written but never run.** bettershot has a ScreenCaptureKit capture backend
that compiles and passes clippy for `aarch64-apple-darwin`, and whose pure
logic is unit-tested — but nobody has executed it on a Mac. Expect problems,
and please report them.

Requires **macOS 14 (Sonoma)** or later: the backend uses
`SCScreenshotManager`, which is a 14.0 API.

### Screen Recording permission

macOS gates screen capture behind TCC. The first capture triggers a system
prompt; if you dismiss it, grant permission manually:

> System Settings → Privacy & Security → Screen & System Audio Recording

**You must quit and reopen bettershot after granting it.** macOS does not hand
the permission to an already-running process, and a process that was denied
will keep seeing empty content until it restarts. bettershot detects that state
and says so rather than reporting a generic failure.

### The menu bar

The tray icon is the menu bar item on macOS. Official builds enable it; if you
build yourself, pass `--features tray`.

Daemon mode does not yet drop out of the Dock — that needs a runtime
`setActivationPolicy(.accessory)` call which has not been written because it
cannot be verified here.

## Troubleshooting

**"no screen capture backend is available"** — you are on a TTY, over SSH
without X forwarding, or in a container with no display. bettershot can still
annotate an existing file: `bettershot --filename shot.png`.

**Capture returns a black image on Wayland** — the portal succeeded but the
compositor refused the content. Check the portal package matches your
compositor.

**The overlay appears in the screenshot** — it should be impossible; bettershot
captures before showing any UI. Please report it with your compositor and
version.

**Text renders as blocks** — no system font was found. Set one explicitly:

```toml
[font]
path = "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf"
```
