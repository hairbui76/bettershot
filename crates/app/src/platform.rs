//! Small per-OS adjustments that have nowhere better to live.
//!
//! Each entry point is a no-op on the platforms it does not apply to, so the
//! call sites stay free of `cfg` noise.

/// Tell the OS this process is a background utility rather than a normal app.
///
/// Only macOS distinguishes the two. Daemon mode has no window until a hotkey
/// or the menu bar item asks for one, and a process with the default
/// `Regular` activation policy sits in the Dock the whole time with nothing
/// behind its icon — clicking it does nothing, and Cmd-Tab offers it as a
/// window-less app. `Accessory` removes it from both while leaving the menu
/// bar item (which *is* the tray on macOS) working, which is what every other
/// menu-bar utility does.
///
/// Called only for daemon mode: a one-shot capture genuinely is a foreground
/// app and should keep its Dock icon for the life of the editor window.
///
/// # Verification status
///
/// Type-checked against the real AppKit API surface for `aarch64-apple-darwin`
/// — `objc2-app-kit` is pure Rust, so `cargo clippy --target` reaches it from
/// a Linux build machine — but **never run on a Mac**, like the rest of the
/// macOS support. See ROADMAP.md.
pub fn become_background_app() {
    #[cfg(target_os = "macos")]
    {
        use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
        use objc2_foundation::MainThreadMarker;

        // AppKit insists this happens on the main thread, and the marker is how
        // objc2 proves it at compile time rather than trusting a comment. This
        // runs before the event loop starts, on the thread that will become the
        // main one, so the marker is normally available; if it somehow is not,
        // a Dock icon is a cosmetic problem and not worth aborting a capture
        // tool over.
        let Some(mtm) = MainThreadMarker::new() else {
            log::warn!("not on the main thread, so the Dock icon stays");
            return;
        };
        let app = NSApplication::sharedApplication(mtm);
        if app.setActivationPolicy(NSApplicationActivationPolicy::Accessory) {
            log::debug!("running as a background app: no Dock icon");
        } else {
            // AppKit returns false rather than erroring when it refuses.
            log::warn!("macOS declined the accessory activation policy");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn becoming_a_background_app_is_safe_to_call_anywhere() {
        // A no-op off macOS, and it must never panic on the platforms where it
        // does nothing — this is called unconditionally on the daemon path.
        become_background_app();
    }
}
