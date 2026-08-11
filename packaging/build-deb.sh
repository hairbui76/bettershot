#!/usr/bin/env bash
# Build a .deb from an already-compiled release binary.
#
# Unlike the Flatpak and AUR manifests in this directory, this script HAS been
# run and its output inspected — see packaging/README.md for which artefacts
# are verified and which are not.
#
#   cargo build --release -p bettershot --features tray
#   packaging/build-deb.sh
#
# The result lands in target/packages/.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT"

VERSION=$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')
ARCH=$(dpkg --print-architecture)
BIN=target/release/bettershot
OUT=target/packages
PKG="$OUT/bettershot_${VERSION}_${ARCH}"

if [[ ! -x "$BIN" ]]; then
  echo "error: $BIN not found. Run: cargo build --release -p bettershot --features tray" >&2
  exit 1
fi

rm -rf "$PKG"
mkdir -p "$PKG"/{DEBIAN,usr/bin,usr/share/applications,usr/share/metainfo}
mkdir -p "$PKG/usr/share/icons/hicolor/scalable/apps"
mkdir -p "$PKG/usr/share/doc/bettershot"

install -Dm755 "$BIN" "$PKG/usr/bin/bettershot"
install -Dm644 assets/org.bettershot.Bettershot.desktop \
  "$PKG/usr/share/applications/org.bettershot.Bettershot.desktop"
install -Dm644 assets/org.bettershot.Bettershot.metainfo.xml \
  "$PKG/usr/share/metainfo/org.bettershot.Bettershot.metainfo.xml"
install -Dm644 assets/bettershot.svg \
  "$PKG/usr/share/icons/hicolor/scalable/apps/org.bettershot.Bettershot.svg"
install -Dm644 LICENSE "$PKG/usr/share/doc/bettershot/copyright"

# The manpage and completions are generated into OUT_DIR by build.rs.
# Several build directories can exist at once (different feature sets, or a
# stale one from an earlier build), so take the most recently written rather
# than whichever the filesystem lists first.
GEN=$(find target/release/build -name 'bettershot.1' -printf '%T@ %h\n' 2>/dev/null \
  | sort -rn | head -1 | cut -d' ' -f2- || true)
if [[ -n "$GEN" ]]; then
  install -Dm644 "$GEN/bettershot.1" "$PKG/usr/share/man/man1/bettershot.1"
  gzip -9n "$PKG/usr/share/man/man1/bettershot.1"
  [[ -f "$GEN/completions/bettershot.bash" ]] && install -Dm644 \
    "$GEN/completions/bettershot.bash" \
    "$PKG/usr/share/bash-completion/completions/bettershot"
  [[ -f "$GEN/completions/bettershot.fish" ]] && install -Dm644 \
    "$GEN/completions/bettershot.fish" \
    "$PKG/usr/share/fish/vendor_completions.d/bettershot.fish"
  [[ -f "$GEN/completions/_bettershot" ]] && install -Dm644 \
    "$GEN/completions/_bettershot" \
    "$PKG/usr/share/zsh/vendor-completions/_bettershot"
fi

# Dependencies are derived from what the binary actually links, rather than
# guessed: a hand-written list drifts the moment a dependency changes.
#
# `dpkg-shlibdeps` is the usual tool but needs a debian/ source tree, which
# this script deliberately does not have. So resolve each linked library to its
# owning package directly.
resolve_deps() {
  local libs pkgs=()
  libs=$(ldd "$PKG/usr/bin/bettershot" 2>/dev/null \
    | awk '/=> \// {print $3}' | sort -u)
  while read -r lib; do
    [[ -z "$lib" ]] && continue
    local owner
    owner=$(dpkg -S "$(readlink -f "$lib")" 2>/dev/null | head -1 | cut -d: -f1) || true
    [[ -n "$owner" ]] && pkgs+=("$owner")
  done <<< "$libs"
  printf '%s\n' "${pkgs[@]}" | sort -u | paste -sd', ' -
}

DEPS=$(resolve_deps)
if [[ -z "$DEPS" ]]; then
  echo "warning: could not resolve any library owners; using a minimal list" >&2
  DEPS="libc6"
fi
echo "dependencies: $DEPS"

INSTALLED_KB=$(du -ks "$PKG" | cut -f1)

cat > "$PKG/DEBIAN/control" <<CONTROL
Package: bettershot
Version: $VERSION
Section: graphics
Priority: optional
Architecture: $ARCH
Depends: $DEPS
Recommends: xdg-desktop-portal, wl-clipboard
Suggests: xdg-desktop-portal-gnome | xdg-desktop-portal-kde | xdg-desktop-portal-wlr
Installed-Size: $INSTALLED_KB
Maintainer: bettershot contributors <noreply@example.invalid>
Homepage: https://github.com/bettershot/bettershot
Description: Modern cross-platform screenshot capture and annotation
 bettershot captures a region, a window or a whole monitor and lets you mark it
 up straight away - arrows, boxes, text, numbered steps, highlights, and blur or
 pixelation for hiding sensitive details - then copies the result to the
 clipboard or saves it to a file.
 .
 Its annotation model is adapted from Satty, whose small and obvious toolset it
 deliberately follows. Unlike Satty it takes the screenshot itself, so there is
 one binary and one hotkey rather than a shell pipeline.
CONTROL

mkdir -p "$OUT"
dpkg-deb --build --root-owner-group "$PKG" >/dev/null
echo "built $PKG.deb"
