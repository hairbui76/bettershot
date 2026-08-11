//! Errors this crate reports before the editor ever opens.
//!
//! These are all things the user typed, so the messages name the offending
//! path or flag and say what was expected: a wrong `--config` path is a typo
//! nine times out of ten, and a stack trace does not fix a typo.

use std::path::PathBuf;

use bettershot_core::config::ConfigError;

#[derive(Debug, thiserror::Error)]
pub enum CliError {
    /// The file named by `--config` is not there. Asked for by name, so its
    /// absence is a mistake rather than the normal "no config yet" case.
    #[error("config file `{}` does not exist", .path.display())]
    MissingConfig { path: PathBuf },

    /// The config file exists but could not be read.
    #[error("could not read config file `{}`: {source}", .path.display())]
    ReadConfig {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// The config file was read but is not valid.
    #[error("config file `{}`: {source}", .path.display())]
    BadConfig {
        path: PathBuf,
        #[source]
        source: ConfigError,
    },

    /// The resolved configuration is internally inconsistent — usually a flag
    /// that undid something the file had set up correctly.
    #[error(transparent)]
    InvalidConfig(#[from] ConfigError),

    /// `--filename` says "annotate this image", `--capture` says "make a new
    /// one". Clap normally catches this; this covers an [`crate::Args`] built
    /// in code.
    #[error(
        "--filename and --capture cannot be used together: pass an image to annotate, or a capture mode to take one"
    )]
    ConflictingInput,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_name_the_offending_path() {
        let error = CliError::MissingConfig {
            path: PathBuf::from("/nope/config.toml"),
        };
        assert!(error.to_string().contains("/nope/config.toml"));

        let error = CliError::BadConfig {
            path: PathBuf::from("/etc/bettershot.toml"),
            source: ConfigError::Parse("expected an equals sign".into()),
        };
        let message = error.to_string();
        assert!(message.contains("/etc/bettershot.toml"), "got: {message}");
        assert!(
            std::error::Error::source(&error).is_some(),
            "the parse error must stay reachable for `{{:#}}` reporting"
        );
    }

    #[test]
    fn the_input_conflict_explains_both_flags() {
        let message = CliError::ConflictingInput.to_string();
        assert!(message.contains("--filename") && message.contains("--capture"));
    }
}
