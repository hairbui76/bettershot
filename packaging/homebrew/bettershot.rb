# Homebrew cask for bettershot.
#
# NOT YET PUBLISHED — see ../README.md. The .app bundle and an unsigned dmg
# are built on every packaging change by .github/workflows/packaging.yml,
# which checks that CFBundleExecutable and CFBundleIconFile resolve inside the
# bundle. Signing and notarization are what remain.
#
# Before this can be submitted to homebrew/cask:
#   1. Build a universal binary (arm64 + x86_64) and wrap it in an .app bundle.
#   2. Sign it with a Developer ID Application certificate.
#   3. Notarize it (`xcrun notarytool submit`) and staple the ticket. Homebrew
#      will not accept an unnotarized cask, and Gatekeeper would refuse to run
#      it anyway.
#   4. Publish the dmg and replace the sha256 below.
#
# macOS capture is NOT implemented yet — see ROADMAP.md Phase 5. This cask
# exists so the packaging is ready, not because there is something to install.

cask "bettershot" do
  version "0.1.0"
  sha256 :no_check # replace with the published dmg's sha256

  url "https://github.com/bettershot/bettershot/releases/download/v#{version}/bettershot-#{version}-universal.dmg",
      verified: "github.com/bettershot/bettershot/"
  name "bettershot"
  desc "Modern cross-platform screenshot capture and annotation"
  homepage "https://github.com/bettershot/bettershot"

  # ScreenCaptureKit needs 12.3; SCScreenshotManager, which bettershot uses,
  # needs 14.0.
  depends_on macos: ">= :sonoma"

  app "bettershot.app"

  binary "#{appdir}/bettershot.app/Contents/MacOS/bettershot", target: "bettershot"

  caveats <<~EOS
    bettershot needs Screen Recording permission before it can capture anything.

    macOS will prompt the first time you try. If you miss the prompt, grant it
    manually:

      System Settings → Privacy & Security → Screen & System Audio Recording

    You must quit and reopen bettershot after granting it; macOS does not hand
    the permission to an already-running process.
  EOS

  zap trash: [
    "~/Library/Application Support/bettershot",
    "~/Library/Preferences/org.bettershot.Bettershot.plist",
    "~/Library/Saved Application State/org.bettershot.Bettershot.savedState",
  ]
end
