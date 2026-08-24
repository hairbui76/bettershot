#!/usr/bin/env bash
# Build an .rpm from an already-compiled release binary.
#
# Like build-deb.sh, this HAS been run and its output inspected. See
# packaging/README.md for which artefacts are verified and which are not.
#
#   cargo build --release -p bettershot --features tray
#   packaging/build-rpm.sh
#
# The result lands in target/packages/.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT"

VERSION=$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')
BIN=target/release/bettershot
OUT="$ROOT/target/packages"
TOP="$ROOT/target/rpmbuild"

if [[ ! -x "$BIN" ]]; then
  echo "error: $BIN not found. Run: cargo build --release -p bettershot --features tray" >&2
  exit 1
fi

rm -rf "$TOP"
mkdir -p "$TOP"/{BUILD,RPMS,SOURCES,SPECS,SRPMS} "$OUT"

# Stage a tarball of exactly what gets installed, which is what %install
# unpacks. Building from the compiled binary rather than from source keeps this
# script honest about what it is: a packaging step, not a build system.
STAGE="$TOP/stage"
mkdir -p "$STAGE"/usr/{bin,share/{applications,metainfo,man/man1}}
mkdir -p "$STAGE/usr/share/licenses/bettershot"

install -Dm755 "$BIN" "$STAGE/usr/bin/bettershot"
install -Dm644 assets/org.bettershot.Bettershot.desktop \
  "$STAGE/usr/share/applications/org.bettershot.Bettershot.desktop"
install -Dm644 assets/org.bettershot.Bettershot.metainfo.xml \
  "$STAGE/usr/share/metainfo/org.bettershot.Bettershot.metainfo.xml"
for size in 16 24 32 48 64 128 256 512; do
  install -Dm644 "assets/icons/bettershot-$size.png" \
    "$STAGE/usr/share/icons/hicolor/${size}x${size}/apps/org.bettershot.Bettershot.png"
done
install -Dm644 LICENSE "$STAGE/usr/share/licenses/bettershot/LICENSE"

# Several build directories can exist at once (different feature sets, or a
# stale one from an earlier build), so take the most recently written rather
# than whichever the filesystem lists first.
GEN=$(find target/release/build -name 'bettershot.1' -printf '%T@ %h\n' 2>/dev/null \
  | sort -rn | head -1 | cut -d' ' -f2- || true)
if [[ -n "$GEN" ]]; then
  install -Dm644 "$GEN/bettershot.1" "$STAGE/usr/share/man/man1/bettershot.1"
  gzip -9n "$STAGE/usr/share/man/man1/bettershot.1"
fi

tar czf "$TOP/SOURCES/bettershot-$VERSION.tar.gz" -C "$STAGE" .

cat > "$TOP/SPECS/bettershot.spec" <<SPEC
Name:           bettershot
Version:        $VERSION
Release:        1%{?dist}
Summary:        Modern cross-platform screenshot capture and annotation
License:        MPL-2.0
URL:            https://github.com/bettershot/bettershot
Source0:        bettershot-$VERSION.tar.gz
BuildArch:      x86_64

# Screen capture goes through the portal on Wayland; the backend is
# compositor-specific, so recommend rather than require any one of them.
Recommends:     xdg-desktop-portal
Suggests:       wl-clipboard

# The binary is already built; there is nothing to compile here.
%global debug_package %{nil}

%description
bettershot captures a region, a window or a whole monitor and lets you mark it
up straight away - arrows, boxes, text, numbered steps, highlights, and blur or
pixelation for hiding sensitive details - then copies the result to the
clipboard or saves it to a file.

Its annotation model is adapted from Satty, whose small and obvious toolset it
deliberately follows. Unlike Satty it takes the screenshot itself, so there is
one binary and one hotkey rather than a shell pipeline.

%prep
%setup -q -c

%build
# Nothing to do: this packages a pre-built binary.

%install
cp -a usr %{buildroot}/

%files
%license /usr/share/licenses/bettershot/LICENSE
/usr/bin/bettershot
/usr/share/applications/org.bettershot.Bettershot.desktop
/usr/share/metainfo/org.bettershot.Bettershot.metainfo.xml
/usr/share/icons/hicolor/*/apps/org.bettershot.Bettershot.png
%{_mandir}/man1/bettershot.1*

%changelog
* Tue Aug 11 2026 bettershot contributors <noreply@example.invalid> - $VERSION-1
- Initial package.
SPEC

rpmbuild --define "_topdir $TOP" -bb "$TOP/SPECS/bettershot.spec" >"$TOP/build.log" 2>&1 || {
  echo "rpmbuild failed; last lines:" >&2
  tail -20 "$TOP/build.log" >&2
  exit 1
}

find "$TOP/RPMS" -name '*.rpm' -exec cp {} "$OUT/" \;
echo "built $(find "$OUT" -name 'bettershot-*.rpm' | head -1)"
