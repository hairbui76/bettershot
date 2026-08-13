# Migrating from Satty

bettershot's annotation model is adapted from [Satty](https://github.com/Satty-org/Satty),
so most of what you know transfers directly. This page covers the differences.

## The big one: you no longer need a separate grabber

Satty annotates an image you hand it:

```sh
grim -g "$(slurp)" - | satty --filename -
```

bettershot can still do exactly that:

```sh
grim -g "$(slurp)" - | bettershot --filename -
```

…but it does not need to, because it captures the screen itself:

```sh
bettershot --capture region
```

That single command replaces the grabber, the region picker, and the annotator.
Rebind your compositor's screenshot key to it and delete the shell pipeline.

## Command-line flags

Flags that exist in both tools keep their names and meanings.

| Satty | bettershot | Notes |
| --- | --- | --- |
| `--filename <PATH>` | `--filename <PATH>` | Same, `-` still means stdin. |
| `--output-filename <PATH>` | `--output-filename <PATH>` | Same, including strftime placeholders. |
| `--early-exit` | `--early-exit` | Same. |
| `--fullscreen` | `--fullscreen` | Same. |
| `--copy-command <CMD>` | `--copy-command <CMD>` | Same. |
| `--annotation-size-factor <F>` | `--annotation-size-factor <F>` | Same. |
| `--init-tool <TOOL>` | `--initial-tool <TOOL>` | `--init-tool` still accepted as an alias. |
| `--default-hide-toolbars` | `--hide-toolbars` | `--default-hide-toolbars` still accepted. |
| `--action-on-enter <A>` | `--action-on-enter <A>` | Same actions. |
| `--no-window-decoration` | `--no-window-decoration` | Same. |
| `--disable-notifications` | `--disable-notifications` | Same. |
| — | `--capture <MODE>` | New: `region`, `window`, `monitor`, `all`. |
| — | `--delay <SECONDS>` | New: for catching menus and hover states. |
| — | `--include-cursor` | New: X11 and Windows; the Wayland portal has no cursor control. |
| — | `--always-on-top <BOOL>` | New: keep the window in front, like the Snipping Tool. On by default. |
| — | `--no-config` | New: ignore the config file entirely. |

## Configuration file

Satty reads `$XDG_CONFIG_HOME/satty/config.toml`. bettershot reads
`$XDG_CONFIG_HOME/bettershot/config.toml` on Linux and
`%APPDATA%\bettershot\config.toml` on Windows.

Most keys carry over unchanged. Differences:

| Satty key | bettershot key | Notes |
| --- | --- | --- |
| `[general]` section | *(no section)* | bettershot's general keys live at the top level. Move them out of `[general]`. |
| `initial-tool` | `initial-tool` | Same values. |
| `default-hide-toolbars` | `hide-toolbars` | Renamed. |
| `default-fill-shapes` | `default-fill-shapes` | Same. |
| `[color-palette] palette = [...]` | same, or a bare `color-palette = [...]` | bettershot accepts either form. |
| `primary-highlighter` | *(not yet implemented)* | Highlight is currently block-only. |
| — | `[capture]` section | New: `mode`, `delay-seconds`, `include-cursor`, `snap-to-windows`. |

A Satty config like:

```toml
[general]
initial-tool = "brush"
default-hide-toolbars = false
output-filename = "/tmp/shot-%Y%m%d.png"

[color-palette]
palette = ["#ff0000", "#00ff00"]
```

becomes:

```toml
initial-tool = "brush"
hide-toolbars = false
output-filename = "/tmp/shot-%Y%m%d.png"
color-palette = ["#ff0000", "#00ff00"]
```

Unknown keys are rejected with an error naming the key, so a half-migrated
config tells you what to fix rather than silently ignoring it.

## Keyboard

Identical, with two additions:

- <kbd>Ctrl</kbd>+<kbd>0</kbd> — zoom to fit.
- <kbd>Enter</kbd> applies the crop while the crop tool is active, instead of
  performing the configured action.

## Behaviour differences worth knowing

- **Obscure (blur) sampling.** In bettershot, blur and pixelate always read the
  original screenshot, so drawing over a blurred region, undoing an annotation
  underneath it, or cropping never changes what the blur hides. The preview on
  screen and the exported file use the same code path.
- **Cropping is undoable.** Satty's crop is applied on save; bettershot applies
  it to the document, rebases every existing annotation onto the new origin,
  and puts it on the undo stack.
- **Numbered markers renumber on undo.** Undoing a marker frees its number, so
  the sequence stays contiguous.
- **Rendering.** Satty uses GTK4 and femtovg/OpenGL; bettershot uses egui and
  wgpu. Annotations are geometrically equivalent but not pixel-identical.

## What Satty still does better today

bettershot is younger. If you rely on these, stay on Satty for now:

- Full IME support for CJK text input (bettershot handles composition, but
  Satty's dedicated input-method integration is more mature).
- The freehand ("primary") highlighter style.
- A settled packaging story across many distributions.

See [ROADMAP.md](../ROADMAP.md) for where these sit.
