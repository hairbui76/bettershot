//! Desktop notifications for save and copy.
//!
//! Notifications are a courtesy, never a dependency: on a headless session, a
//! machine with no notification daemon, or a locked-down desktop, sending one
//! fails and that must not turn a successful save into a reported failure. So
//! every function here swallows errors down to a log line.

use bettershot_core::config::Config;

/// Application name shown by the notification daemon.
#[cfg(all(
    feature = "notifications",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
const APP_NAME: &str = "bettershot";

/// Tell the user something succeeded.
pub fn notify(config: &Config, summary: &str, body: &str) {
    if config.disable_notifications {
        return;
    }
    if let Err(e) = send(summary, body) {
        log::debug!("could not show a notification: {e}");
    }
}

/// Notify that an image was written, naming the file.
pub fn saved(config: &Config, path: &std::path::Path) {
    notify(config, "Screenshot saved", &path.display().to_string());
}

/// Notify that the image went to the clipboard.
pub fn copied(config: &Config) {
    notify(
        config,
        "Screenshot copied",
        "The image is on the clipboard.",
    );
}

#[cfg(all(
    feature = "notifications",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
fn send(summary: &str, body: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut notification = notify_rust::Notification::new();
    notification
        .appname(APP_NAME)
        .summary(summary)
        .body(body)
        .icon("bettershot");

    // Freedesktop-only hints; setting them elsewhere does not compile.
    #[cfg(all(unix, not(target_os = "macos")))]
    notification
        .hint(notify_rust::Hint::Category("transfer.complete".to_owned()))
        .timeout(notify_rust::Timeout::Milliseconds(4000));

    notification.show()?;
    Ok(())
}

#[cfg(not(all(
    feature = "notifications",
    any(target_os = "linux", target_os = "macos", target_os = "windows")
)))]
fn send(_summary: &str, _body: &str) -> Result<(), Box<dyn std::error::Error>> {
    Err("this build has no notification support".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notifications_are_skipped_when_disabled() {
        let config = Config {
            disable_notifications: true,
            ..Default::default()
        };
        // The assertion is that this neither panics nor blocks; with
        // notifications disabled it must not even attempt to contact a daemon.
        notify(&config, "summary", "body");
    }

    #[test]
    fn a_failing_notification_never_panics() {
        // There is no notification daemon in CI or on a headless machine, so
        // this exercises the error path directly.
        let config = Config::default();
        notify(&config, "summary", "body");
        copied(&config);
        saved(&config, std::path::Path::new("/tmp/example.png"));
    }
}
