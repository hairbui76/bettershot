# Packaging

Manifests for the distribution channels bettershot targets.

## What is actually verified

| Artefact | Built? | Validated? |
| --- | --- | --- |
| **`.deb`** (`build-deb.sh`) | **yes** | **yes** — extracts, the binary runs from the package tree, dependencies resolved from the real link set |
| **`.rpm`** (`build-rpm.sh`) | **yes** | **yes** — `rpm -qip`/`-qlp` inspected, extracts, the binary runs from the package tree |
| `assets/*.desktop` | n/a | **yes** — `desktop-file-validate`, clean |
| `assets/*.metainfo.xml` | n/a | **yes** — `appstreamcli validate`, clean |
| Flatpak manifest | no | syntax only |
| AUR `PKGBUILD` | no | syntax only (`bash -n`) |
| winget manifest | no | syntax only |
| WiX `.wxs` | no | well-formed XML only |
| Homebrew cask | no | not checked (no ruby available) |

Build and check them yourself:

```sh
cargo build --release -p bettershot --features tray
packaging/build-deb.sh && dpkg-deb -I target/packages/bettershot_*.deb
packaging/build-rpm.sh && rpm -qip target/packages/bettershot-*.rpm
```

## Status of the rest: authored, not yet built

**Apart from the `.deb` and `.rpm`, none of these has been built or installed.** They were written against each
format's documented schema and are **syntax-checked** — the Flatpak and winget
YAML parse (and winget's three documents are in the order it expects), the
`PKGBUILD` passes `bash -n`, and the WiX and AppStream XML are well-formed —
but no Flatpak was produced, no MSI compiled, no package installed. Treat them
as a reviewed starting point that a packager still has to run, not as finished
artefacts.

Where a step needs a credential this repository does not hold — an Authenticode
certificate, an Apple Developer ID, a winget-pkgs or AUR account — that is
called out inline. Those steps cannot be automated here and should not be
faked.

| File | Channel | What still has to happen |
| --- | --- | --- |
| `flatpak/org.bettershot.Bettershot.yml` | Flathub | Generate `cargo-sources.json` with `flatpak-cargo-generator.py`, then `flatpak-builder`. Flathub requires offline builds, so vendored sources are mandatory. |
| `aur/PKGBUILD` | Arch User Repository | Fill in `sha256sums`, build in a clean chroot, submit with an AUR account. |
| `winget/manifest.yaml` | winget | Sign the installer, publish a release, compute its `InstallerSha256`, submit a PR to `microsoft/winget-pkgs`. |
| `windows/bettershot.wxs` | MSI | Build with WiX v4, then sign with `signtool` and an Authenticode certificate. |
| `homebrew/bettershot.rb` | Homebrew cask | Notarize the app bundle with an Apple Developer ID, publish, compute the sha256. |

Unsigned archives and shell/PowerShell installers for every target are
configured declaratively in the root `Cargo.toml` under
`[workspace.metadata.dist]`. That path needs no credentials and is the one to
use until the signed channels are set up.

**That config has not been validated by running `cargo dist plan`** — building
`cargo-dist` failed twice in the development container for reasons unrelated to
bettershot. Run it before relying on the config:

```sh
cargo install cargo-dist
cargo dist plan
```

A tag-triggered workflow in `.github/workflows/release.yml` already builds and
uploads archives for Linux, Windows and both macOS architectures without
needing `cargo-dist` at all, and creates the release as a **draft** so nothing
is published before a human has signed the installers and walked
`../docs/release-checklist.md`.

## Flathub needs screenshots, and they cannot be faked here

AppStream metainfo for Flathub must include `<screenshots>` — real pictures of
the application running. `assets/org.bettershot.Bettershot.metainfo.xml` has
none, because there is no desktop session here to take them on.

Do not substitute a rendering produced by `cargo run -p bettershot-render
--example showcase`. That image shows what the *rasterizer* can draw, not what
the *application* looks like, and presenting it as a screenshot would misinform
anyone browsing the store. Take real ones when the app first runs on a desktop,
add them to the metainfo, and validate with `appstreamcli validate`.

## The Linux build needs GTK only for the tray

`bettershot` links GTK 3 solely for the system-tray icon, which is behind the
`tray` cargo feature. Distribution builds enable it, which is why the manifests
list `gtk3` and `libayatana-appindicator`. A build without `--features tray`
drops both dependencies entirely.
