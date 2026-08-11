//! The bettershot argument surface and configuration loader.
//!
//! This is a **library**, not the binary: the app crate uses it at runtime to
//! turn `argv` plus a config file into a resolved [`Config`], and a `build.rs`
//! uses [`command()`] to generate shell completions and a manpage without
//! linking the GUI. That is the same split Satty uses (`Satty/cli`), and it is
//! why nothing here touches a window.
//!
//! # Precedence
//!
//! **CLI argument > config file > built-in default.** Every layer is folded
//! onto the previous one and only overrides the keys it actually mentions, so
//! a config file that sets one key leaves the rest alone, and a flag that was
//! not passed never clobbers the file.
//!
//! Booleans are the classic trap here: a plain `bool` field is `false` when
//! absent, which is indistinguishable from `--flag=false` and would silently
//! reset a `true` from the config file. Every boolean flag is therefore an
//! `Option<bool>` accepting an optional value — `--fullscreen`,
//! `--fullscreen=true` and `--fullscreen=false` are all spellings of "the user
//! said something", and `None` means "the user said nothing".
//!
//! # Where the config file lives
//!
//! | Platform | Path |
//! | --- | --- |
//! | Linux/BSD | `$XDG_CONFIG_HOME/bettershot/config.toml`, else `~/.config/bettershot/config.toml` |
//! | Windows | `%APPDATA%\bettershot\config.toml` |
//! | macOS | `~/Library/Application Support/bettershot/config.toml` |
//!
//! `--config <PATH>` overrides discovery; `--no-config` skips the file layer
//! entirely, which is what scripts and tests want for reproducibility. A
//! discovered file that does not exist is normal and silent; a file asked for
//! by name that does not exist is an error, as is one that fails to parse.
//!
//! # Typical use
//!
//! ```no_run
//! use bettershot_cli::{Args, InputSource, Parser as _};
//!
//! let args = Args::parse();
//! let config = bettershot_cli::load_config(&args)?;
//! match bettershot_cli::input_source(&args, &config)? {
//!     InputSource::Stdin => { /* decode an image from stdin */ }
//!     InputSource::File(path) => { /* open `path` */ }
//!     InputSource::Capture(mode) => { /* grab the screen in `mode` */ }
//! }
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

pub mod args;
pub mod error;
pub mod filename;
pub mod input;
pub mod loader;

pub use args::{Args, BIN_NAME, command, render_completions, render_manpage};
pub use error::CliError;
pub use filename::{expand_filename, resolve_output_path};
pub use input::{InputSource, input_source};
pub use loader::{apply_args, config_path, load_config, load_config_with};

/// Re-exported so callers need not depend on `clap` themselves just to call
/// [`Args::parse`] or [`command()`].
pub use clap::{CommandFactory, Parser};
/// Re-exported for `--completions <SHELL>` and for a `build.rs` generating
/// completion scripts.
pub use clap_complete::Shell;

/// The project licence, printed by `--license`.
///
/// Embedded from the workspace root because a `cargo install`ed binary has no
/// packaged files next to it — the same reason Satty carries `--license`.
pub const LICENSE_TEXT: &str = include_str!("../../../LICENSE");
