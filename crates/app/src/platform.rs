//! Small per-OS adjustments that have nowhere better to live.
//!
//! Each entry point is a no-op on the platforms it does not apply to, so the
//! call sites stay free of `cfg` noise.
#![cfg_attr(target_os = "windows", allow(unsafe_code))]

/// Re-attach to the terminal that launched us, if there was one.
///
/// bettershot is built for the Windows subsystem so that launching it does not
/// flash up a console -- see the note at the top of `main.rs`. The side effect
/// is that the process starts with no standard handles, so anything printed
/// goes nowhere: `--help`, `--man`, `--license`, shell completions and
/// `--output-filename -` would all appear to do nothing when run from a
/// terminal.
///
/// `AttachConsole(ATTACH_PARENT_PROCESS)` borrows the parent's console when
/// there is one, restoring all of that, and fails harmlessly when there is not
/// -- which is the normal case for a Start-menu shortcut or the login
/// autostart, and exactly when a console *should* not appear.
///
/// A no-op everywhere else: only Windows distinguishes the two subsystems.
pub fn attach_parent_console() {
    #[cfg(target_os = "windows")]
    {
        // SAFETY: a plain FFI call taking a constant and touching no memory of
        // ours. It returns 0 when there is no parent console, which is not an
        // error worth reporting -- there is nowhere to report it to.
        unsafe {
            windows_sys::Win32::System::Console::AttachConsole(
                windows_sys::Win32::System::Console::ATTACH_PARENT_PROCESS,
            );
        }
    }
}

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
