# Changelog

## [0.7.0](https://github.com/hairbui76/bettershot/compare/v0.6.1...v0.7.0) (2026-08-24)


### Features

* ship a Windows installer, and let hotkeys be set from Settings ([043a465](https://github.com/hairbui76/bettershot/commit/043a46535da32d604c9dabdfb3c326dceb6fa444))

## [0.6.1](https://github.com/hairbui76/bettershot/compare/v0.6.0...v0.6.1) (2026-08-19)


### Bug Fixes

* **daemon:** only announce the daemon once it is actually starting ([f9d4ede](https://github.com/hairbui76/bettershot/commit/f9d4edef48fbe103d096db245ff654820896ecaf))
* **daemon:** say out loud whether the hotkeys registered ([0472ac9](https://github.com/hairbui76/bettershot/commit/0472ac9337e065a89d3d364596a44456870676e6))

## [0.6.0](https://github.com/hairbui76/bettershot/compare/v0.5.0...v0.6.0) (2026-08-17)


### Features

* **app:** floating capture-mode bar on the selection overlay ([a8d6452](https://github.com/hairbui76/bettershot/commit/a8d6452ae74c01346bb820af160d152d070d3cb7))

## [0.5.0](https://github.com/hairbui76/bettershot/compare/v0.4.0...v0.5.0) (2026-08-13)


### Features

* **app:** keep the window on top, like the Windows Snipping Tool ([04266c1](https://github.com/hairbui76/bettershot/commit/04266c1d87df7ecb5a203c7a82c81e05193140fa))

## [0.4.0](https://github.com/hairbui76/bettershot/compare/v0.3.0...v0.4.0) (2026-08-12)


### Features

* **app:** editor, selection overlay and resident daemon ([705c4bc](https://github.com/hairbui76/bettershot/commit/705c4bcd5263ab4c17a384bc048bf52894028f12))
* **capture:** implement --include-cursor, on X11 for now ([c306299](https://github.com/hairbui76/bettershot/commit/c3062992d73c240052386e0ffea033e1d77edb2f))
* **capture:** screen capture for Linux, Windows and macOS ([ddd22e7](https://github.com/hairbui76/bettershot/commit/ddd22e71b9a98e5566d36633a1f641c398b4b547))
* **cli:** argument parsing and configuration layering ([fa4d024](https://github.com/hairbui76/bettershot/commit/fa4d024f6e0333f0c5b6aaaec1eab03076483fa9))
* **core:** platform-agnostic annotation model ([9e6d25a](https://github.com/hairbui76/bettershot/commit/9e6d25aefa95fb8b4ce987f7b818b58f0606713a))
* **macos:** run daemon mode as an accessory app so it leaves the Dock ([1bf13de](https://github.com/hairbui76/bettershot/commit/1bf13de122b835fdb72d9b754b5d5194f79b2fe3))
* **render:** CPU rasterizer for export and headless testing ([604f2e6](https://github.com/hairbui76/bettershot/commit/604f2e6d320ade74b73ef9204f090cc526f350c4))
* **windows:** implement --include-cursor via Win32 ([b5f71cd](https://github.com/hairbui76/bettershot/commit/b5f71cdf2dc3a9056e7d28e0de9847df83130dfc))


### Bug Fixes

* **assets:** put the svg element first so the icon loads as an image ([3ff5c60](https://github.com/hairbui76/bettershot/commit/3ff5c60fe20bfec3ad1e6c24efc8c5327a0c8995))
* **build:** the tray needs libxdo on Linux ([c7486c7](https://github.com/hairbui76/bettershot/commit/c7486c7d2506197302ddd807e0594c44912ad2a6))
* **capture:** harden the cursor blend after an adversarial review ([4a04ccc](https://github.com/hairbui76/bettershot/commit/4a04ccc3ccc7c35b09a45cdde61adaf75688d5f0))
* **ci:** pin the toolchain and clear the lints it exposed ([8403b32](https://github.com/hairbui76/bettershot/commit/8403b327509e6c0db937e0b47fe4b1440c054df8))
* **ci:** stop release-please re-releasing, and actually attach the binaries ([ad2d7b0](https://github.com/hairbui76/bettershot/commit/ad2d7b09a3679738d6a4faba55dc2f432e9a2a80))
* **flatpak:** give Rust the library search path for libxdo ([28c9f4b](https://github.com/hairbui76/bettershot/commit/28c9f4b58220bcd283d75b610f8f51b807a69b53))
* **macos:** convert ScreenCaptureKit's premultiplied pixels to straight alpha ([4ec304f](https://github.com/hairbui76/bettershot/commit/4ec304fcae4c50c0f675e979c538b6e513209c58))
* modified keys typed into text, and a Windows-only lint ([0cfe4c8](https://github.com/hairbui76/bettershot/commit/0cfe4c8024665d9632c80e0dc6889039731a1155))
* **packaging:** make the MSI actually build ([37768fb](https://github.com/hairbui76/bettershot/commit/37768fbeddcddbf5676bdd51915526d744130fb1))
* **packaging:** resolve MSI source paths from the repository root ([9dddeef](https://github.com/hairbui76/bettershot/commit/9dddeef5093054409855a028b7dba43f554b9e47))
* **packaging:** the wxs header was not valid XML ([95c5a60](https://github.com/hairbui76/bettershot/commit/95c5a60885b4eac21a79a9bea146ce2e0e794030))

## [0.3.0](https://github.com/hairbui76/bettershot/compare/v0.2.0...v0.3.0) (2026-08-12)


### Features

* **app:** editor, selection overlay and resident daemon ([705c4bc](https://github.com/hairbui76/bettershot/commit/705c4bcd5263ab4c17a384bc048bf52894028f12))
* **capture:** implement --include-cursor, on X11 for now ([c306299](https://github.com/hairbui76/bettershot/commit/c3062992d73c240052386e0ffea033e1d77edb2f))
* **capture:** screen capture for Linux, Windows and macOS ([ddd22e7](https://github.com/hairbui76/bettershot/commit/ddd22e71b9a98e5566d36633a1f641c398b4b547))
* **cli:** argument parsing and configuration layering ([fa4d024](https://github.com/hairbui76/bettershot/commit/fa4d024f6e0333f0c5b6aaaec1eab03076483fa9))
* **core:** platform-agnostic annotation model ([9e6d25a](https://github.com/hairbui76/bettershot/commit/9e6d25aefa95fb8b4ce987f7b818b58f0606713a))
* **macos:** run daemon mode as an accessory app so it leaves the Dock ([1bf13de](https://github.com/hairbui76/bettershot/commit/1bf13de122b835fdb72d9b754b5d5194f79b2fe3))
* **render:** CPU rasterizer for export and headless testing ([604f2e6](https://github.com/hairbui76/bettershot/commit/604f2e6d320ade74b73ef9204f090cc526f350c4))
* **windows:** implement --include-cursor via Win32 ([b5f71cd](https://github.com/hairbui76/bettershot/commit/b5f71cdf2dc3a9056e7d28e0de9847df83130dfc))


### Bug Fixes

* **assets:** put the svg element first so the icon loads as an image ([3ff5c60](https://github.com/hairbui76/bettershot/commit/3ff5c60fe20bfec3ad1e6c24efc8c5327a0c8995))
* **build:** the tray needs libxdo on Linux ([c7486c7](https://github.com/hairbui76/bettershot/commit/c7486c7d2506197302ddd807e0594c44912ad2a6))
* **capture:** harden the cursor blend after an adversarial review ([4a04ccc](https://github.com/hairbui76/bettershot/commit/4a04ccc3ccc7c35b09a45cdde61adaf75688d5f0))
* **ci:** pin the toolchain and clear the lints it exposed ([8403b32](https://github.com/hairbui76/bettershot/commit/8403b327509e6c0db937e0b47fe4b1440c054df8))
* **flatpak:** give Rust the library search path for libxdo ([28c9f4b](https://github.com/hairbui76/bettershot/commit/28c9f4b58220bcd283d75b610f8f51b807a69b53))
* **macos:** convert ScreenCaptureKit's premultiplied pixels to straight alpha ([4ec304f](https://github.com/hairbui76/bettershot/commit/4ec304fcae4c50c0f675e979c538b6e513209c58))
* modified keys typed into text, and a Windows-only lint ([0cfe4c8](https://github.com/hairbui76/bettershot/commit/0cfe4c8024665d9632c80e0dc6889039731a1155))
* **packaging:** make the MSI actually build ([37768fb](https://github.com/hairbui76/bettershot/commit/37768fbeddcddbf5676bdd51915526d744130fb1))
* **packaging:** resolve MSI source paths from the repository root ([9dddeef](https://github.com/hairbui76/bettershot/commit/9dddeef5093054409855a028b7dba43f554b9e47))
* **packaging:** the wxs header was not valid XML ([95c5a60](https://github.com/hairbui76/bettershot/commit/95c5a60885b4eac21a79a9bea146ce2e0e794030))

## [0.2.0](https://github.com/hairbui76/bettershot/compare/v0.1.0...v0.2.0) (2026-08-12)


### Features

* **app:** editor, selection overlay and resident daemon ([705c4bc](https://github.com/hairbui76/bettershot/commit/705c4bcd5263ab4c17a384bc048bf52894028f12))
* **capture:** implement --include-cursor, on X11 for now ([c306299](https://github.com/hairbui76/bettershot/commit/c3062992d73c240052386e0ffea033e1d77edb2f))
* **capture:** screen capture for Linux, Windows and macOS ([ddd22e7](https://github.com/hairbui76/bettershot/commit/ddd22e71b9a98e5566d36633a1f641c398b4b547))
* **cli:** argument parsing and configuration layering ([fa4d024](https://github.com/hairbui76/bettershot/commit/fa4d024f6e0333f0c5b6aaaec1eab03076483fa9))
* **core:** platform-agnostic annotation model ([9e6d25a](https://github.com/hairbui76/bettershot/commit/9e6d25aefa95fb8b4ce987f7b818b58f0606713a))
* **macos:** run daemon mode as an accessory app so it leaves the Dock ([1bf13de](https://github.com/hairbui76/bettershot/commit/1bf13de122b835fdb72d9b754b5d5194f79b2fe3))
* **render:** CPU rasterizer for export and headless testing ([604f2e6](https://github.com/hairbui76/bettershot/commit/604f2e6d320ade74b73ef9204f090cc526f350c4))
* **windows:** implement --include-cursor via Win32 ([b5f71cd](https://github.com/hairbui76/bettershot/commit/b5f71cdf2dc3a9056e7d28e0de9847df83130dfc))


### Bug Fixes

* **assets:** put the svg element first so the icon loads as an image ([3ff5c60](https://github.com/hairbui76/bettershot/commit/3ff5c60fe20bfec3ad1e6c24efc8c5327a0c8995))
* **build:** the tray needs libxdo on Linux ([c7486c7](https://github.com/hairbui76/bettershot/commit/c7486c7d2506197302ddd807e0594c44912ad2a6))
* **capture:** harden the cursor blend after an adversarial review ([4a04ccc](https://github.com/hairbui76/bettershot/commit/4a04ccc3ccc7c35b09a45cdde61adaf75688d5f0))
* **ci:** pin the toolchain and clear the lints it exposed ([8403b32](https://github.com/hairbui76/bettershot/commit/8403b327509e6c0db937e0b47fe4b1440c054df8))
* **flatpak:** give Rust the library search path for libxdo ([28c9f4b](https://github.com/hairbui76/bettershot/commit/28c9f4b58220bcd283d75b610f8f51b807a69b53))
* **macos:** convert ScreenCaptureKit's premultiplied pixels to straight alpha ([4ec304f](https://github.com/hairbui76/bettershot/commit/4ec304fcae4c50c0f675e979c538b6e513209c58))
* modified keys typed into text, and a Windows-only lint ([0cfe4c8](https://github.com/hairbui76/bettershot/commit/0cfe4c8024665d9632c80e0dc6889039731a1155))
* **packaging:** make the MSI actually build ([37768fb](https://github.com/hairbui76/bettershot/commit/37768fbeddcddbf5676bdd51915526d744130fb1))
* **packaging:** resolve MSI source paths from the repository root ([9dddeef](https://github.com/hairbui76/bettershot/commit/9dddeef5093054409855a028b7dba43f554b9e47))
* **packaging:** the wxs header was not valid XML ([95c5a60](https://github.com/hairbui76/bettershot/commit/95c5a60885b4eac21a79a9bea146ce2e0e794030))
