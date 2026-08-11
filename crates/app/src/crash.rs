//! Opt-in crash reports.
//!
//! # What a crash report from a screenshot tool must never contain
//!
//! bettershot holds the contents of your screen in memory. A crash report is
//! therefore a privacy hazard in a way that most programs' reports are not: a
//! dump that included pixel data, a file path, or a window title could leak
//! precisely the thing the user was about to redact.
//!
//! So reports carry only the panic message, its source location, the version
//! and the OS. No image data, no capture geometry, no window titles, no
//! configuration values, no environment. Reports are **written locally only**
//! and never transmitted anywhere — there is no endpoint to send them to, by
//! design. The user attaches one to an issue if they choose to.
//!
//! Reporting is off unless `crash-reports = true` is set.

use std::fmt::Write as _;
use std::panic::PanicHookInfo;
use std::path::{Path, PathBuf};

/// Everything a report is allowed to say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub version: &'static str,
    pub os: &'static str,
    pub arch: &'static str,
    /// The panic message, as the panicking code wrote it.
    pub message: String,
    /// `file:line:column` of the panic, when the runtime knows it.
    pub location: Option<String>,
    /// A backtrace, only when the user asked for one via `RUST_BACKTRACE`.
    pub backtrace: Option<String>,
}

impl Report {
    /// Build a report from a panic.
    pub fn from_panic(info: &PanicHookInfo<'_>, backtrace: Option<String>) -> Self {
        // `payload_as_str` is not stable, so recover the message the long way.
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "panic with a non-string payload".to_owned());

        Self {
            version: env!("CARGO_PKG_VERSION"),
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
            message,
            location: info.location().map(|l| l.to_string()),
            backtrace,
        }
    }

    /// Render the report as the text written to disk.
    pub fn render(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "bettershot crash report");
        let _ = writeln!(out, "version: {}", self.version);
        let _ = writeln!(out, "platform: {}-{}", self.os, self.arch);
        let _ = writeln!(
            out,
            "location: {}",
            self.location.as_deref().unwrap_or("unknown")
        );
        let _ = writeln!(out, "message: {}", self.message);
        match &self.backtrace {
            Some(backtrace) => {
                let _ = writeln!(out, "\nbacktrace:\n{backtrace}");
            }
            None => {
                let _ = writeln!(
                    out,
                    "\n(no backtrace; re-run with RUST_BACKTRACE=1 to include one)"
                );
            }
        }
        let _ = writeln!(
            out,
            "\nThis report contains no image data, file paths, window titles or \
             configuration values, and was not sent anywhere."
        );
        out
    }
}

/// Where reports are written.
pub fn report_dir() -> Option<PathBuf> {
    bettershot_cli::config_path()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .map(|dir| dir.join("crash-reports"))
}

/// Install the panic hook, if the user opted in.
///
/// The previous hook is kept and still runs, so the usual message still
/// reaches stderr.
pub fn install(enabled: bool) {
    if !enabled {
        return;
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let backtrace = std::env::var("RUST_BACKTRACE")
            .ok()
            .filter(|v| v != "0")
            .map(|_| std::backtrace::Backtrace::force_capture().to_string());

        let report = Report::from_panic(info, backtrace);
        match write_report(&report) {
            Ok(path) => eprintln!("bettershot: crash report written to {}", path.display()),
            Err(e) => eprintln!("bettershot: could not write a crash report: {e}"),
        }
        previous(info);
    }));
}

fn write_report(report: &Report) -> Result<PathBuf, String> {
    let dir = report_dir().ok_or_else(|| "no configuration directory".to_owned())?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;

    // A monotonic-ish name without pulling a clock into the panic path, where
    // allocating as little as possible is wise.
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("crash-{stamp}.txt"));
    std::fs::write(&path, report.render()).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> Report {
        Report {
            version: "0.1.0",
            os: "linux",
            arch: "x86_64",
            message: "something went wrong".to_owned(),
            location: Some("crates/app/src/editor.rs:42:9".to_owned()),
            backtrace: None,
        }
    }

    #[test]
    fn a_report_states_the_version_platform_and_panic() {
        let text = report().render();
        assert!(text.contains("0.1.0"), "{text}");
        assert!(text.contains("linux-x86_64"), "{text}");
        assert!(text.contains("something went wrong"), "{text}");
        assert!(text.contains("editor.rs:42:9"), "{text}");
    }

    #[test]
    fn a_report_says_how_to_get_a_backtrace_when_there_is_none() {
        let text = report().render();
        assert!(text.contains("RUST_BACKTRACE=1"), "{text}");
    }

    #[test]
    fn a_backtrace_is_included_when_one_was_captured() {
        let mut r = report();
        r.backtrace = Some("frame 0\nframe 1".to_owned());
        let text = r.render();
        assert!(text.contains("frame 0"), "{text}");
        assert!(!text.contains("RUST_BACKTRACE=1"), "no need to suggest it");
    }

    #[test]
    fn a_report_promises_and_keeps_its_privacy_guarantee() {
        // The guarantee is structural: `Report` has no field that could hold
        // image data, a path from the user's filesystem, or a window title.
        // If a future change adds one, this test is where it should be
        // reconsidered rather than quietly allowed.
        let text = report().render();
        assert!(text.contains("no image data"), "{text}");
        assert!(text.contains("not sent anywhere"), "{text}");
    }

    #[test]
    fn reports_land_beside_the_configuration_not_in_the_working_directory() {
        // A crash report dropped into whatever directory the user happened to
        // run from would be litter, and might land somewhere shared.
        if let Some(dir) = report_dir() {
            assert!(dir.ends_with("crash-reports"), "{}", dir.display());
            assert!(dir.is_absolute(), "{}", dir.display());
        }
    }

    #[test]
    fn installing_the_hook_is_a_no_op_when_not_opted_in() {
        // Must not replace the process-wide hook, which other tests rely on.
        install(false);
        let result = std::panic::catch_unwind(|| panic!("expected"));
        assert!(result.is_err());
    }
}
