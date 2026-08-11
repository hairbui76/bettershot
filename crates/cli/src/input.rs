//! Deciding where the image comes from.

use std::path::{Path, PathBuf};

use bettershot_core::Config;
use bettershot_core::config::CaptureMode;

use crate::args::Args;
use crate::error::CliError;

/// Where the app should get the image to annotate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputSource {
    /// An existing image file.
    File(PathBuf),
    /// An image piped in, as in `grim - | bettershot -f -`.
    Stdin,
    /// No image yet: take one.
    Capture(CaptureMode),
}

/// Resolve the input source from the arguments, falling back to the configured
/// capture mode.
///
/// Capturing is the default because bettershot is a screenshot tool first:
/// running it with no arguments should grab the screen, not complain. (This is
/// the one place bettershot deliberately differs from Satty, whose
/// `--filename` is required because it has no capture of its own.)
///
/// `--filename` and `--capture` are mutually exclusive; clap rejects the
/// combination before this is reached, so the error here is for an [`Args`]
/// built in code.
pub fn input_source(args: &Args, config: &Config) -> Result<InputSource, CliError> {
    match (args.filename.as_deref(), args.capture) {
        (Some(_), Some(_)) => Err(CliError::ConflictingInput),
        (Some(path), None) if path == Path::new("-") => Ok(InputSource::Stdin),
        (Some(path), None) => Ok(InputSource::File(path.to_path_buf())),
        (None, Some(mode)) => Ok(InputSource::Capture(mode)),
        (None, None) => Ok(InputSource::Capture(config.capture.mode)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser as _;

    fn args(argv: &[&str]) -> Args {
        Args::try_parse_from(argv).expect("should parse")
    }

    fn source(argv: &[&str]) -> InputSource {
        input_source(&args(argv), &Config::default()).expect("should resolve")
    }

    #[test]
    fn a_dash_filename_means_stdin() {
        assert_eq!(
            source(&["bettershot", "--filename", "-"]),
            InputSource::Stdin
        );
        assert_eq!(source(&["bettershot", "-f", "-"]), InputSource::Stdin);
    }

    #[test]
    fn a_filename_means_that_file() {
        assert_eq!(
            source(&["bettershot", "--filename", "/tmp/shot.png"]),
            InputSource::File(PathBuf::from("/tmp/shot.png"))
        );
        // A file that happens to be named like the stdin marker still needs a
        // path to disambiguate it, which is the usual convention.
        assert_eq!(
            source(&["bettershot", "--filename", "./-"]),
            InputSource::File(PathBuf::from("./-"))
        );
    }

    #[test]
    fn a_capture_mode_means_capture() {
        for (flag, mode) in [
            ("region", CaptureMode::Region),
            ("window", CaptureMode::Window),
            ("monitor", CaptureMode::Monitor),
            ("all", CaptureMode::All),
        ] {
            assert_eq!(
                source(&["bettershot", "--capture", flag]),
                InputSource::Capture(mode)
            );
        }
    }

    #[test]
    fn no_input_flags_falls_back_to_the_configured_capture_mode() {
        let config = Config {
            capture: bettershot_core::config::CaptureConfig {
                mode: CaptureMode::Monitor,
                ..Default::default()
            },
            ..Config::default()
        };
        assert_eq!(
            input_source(&args(&["bettershot"]), &config).expect("should resolve"),
            InputSource::Capture(CaptureMode::Monitor)
        );
        // And with an untouched config, the default mode.
        assert_eq!(
            source(&["bettershot"]),
            InputSource::Capture(CaptureMode::Region)
        );
    }

    #[test]
    fn asking_for_both_a_file_and_a_capture_is_an_error() {
        // Clap catches this on the command line...
        assert!(
            Args::try_parse_from(["bettershot", "-f", "a.png", "-c", "region"]).is_err(),
            "clap should reject the combination"
        );
        // ...and the resolver catches it for an `Args` built in code.
        let args = Args {
            filename: Some(PathBuf::from("a.png")),
            capture: Some(CaptureMode::Region),
            ..Args::default()
        };
        assert!(matches!(
            input_source(&args, &Config::default()),
            Err(CliError::ConflictingInput)
        ));
    }
}
