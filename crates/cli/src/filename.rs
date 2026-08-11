//! Output filename templating.
//!
//! `--output-filename '~/shots/%Y%m%d-%H%M%S.png'` has to become a real path
//! at save time. Two expansions happen, in this order:
//!
//! 1. **strftime placeholders**, using [`chrono`]'s vocabulary, so the same
//!    syntax people already use with `grim`, `scrot` and Satty works here.
//! 2. **a leading `~`**, expanded to the home directory — the shell never got
//!    a chance to do it, because the template usually arrives quoted from a
//!    keybinding or a config file.
//!
//! The timestamp is a parameter rather than a call to `Local::now()` inside,
//! which is what makes this testable.

use std::fmt::Write as _;
use std::path::PathBuf;

use bettershot_core::Config;
use bettershot_core::config::SaveFormat;
use chrono::format::{Item, StrftimeItems};
use chrono::{DateTime, Local};

/// Extensions we are willing to recognise as "this filename already says what
/// format it is". Anything else is treated as part of the name, so
/// `report.2026.final` gains an extension rather than losing its tail.
const KNOWN_EXTENSIONS: [&str; 4] = ["png", "jpg", "jpeg", "webp"];

/// The conventional stand-in for stdout, passed through untouched.
const STDOUT: &str = "-";

/// Expand strftime placeholders and a leading `~` in `template`.
///
/// Placeholders chrono does not understand are left exactly as written, so a
/// literal percent in a filename (`100%.png`) survives instead of turning into
/// an error or a mangled path. `%%` is the explicit escape and yields `%`.
pub fn expand_filename(template: &str, now: DateTime<Local>) -> String {
    expand_tilde(&expand_strftime(template, now))
}

/// The path a save action should write to: [`expand_filename`] over
/// `config.output_filename`, with the extension made to agree with
/// `config.save_format`.
///
/// `None` when no output filename is configured, which means "saving to a file
/// is disabled" rather than "save somewhere default" — writing to a guessed
/// path without being asked is not a favour.
pub fn resolve_output_path(config: &Config, now: DateTime<Local>) -> Option<PathBuf> {
    let template = config.output_filename.as_deref()?;
    let expanded = expand_filename(template, now);
    if expanded == STDOUT {
        return Some(PathBuf::from(STDOUT));
    }
    Some(with_extension(PathBuf::from(expanded), config.save_format))
}

/// Give `path` an extension if it has none.
///
/// An extension the user actually typed **wins**: `--output-filename shot.jpg`
/// means JPEG, whatever `save-format` says, because naming the file is the more
/// specific instruction and it is what every other screenshot tool does. This
/// used to rewrite `.jpg` to `.png` whenever `save-format` was left at its
/// default, so the file was named `.png` and the request for JPEG was lost —
/// silently, since PNG *is* the default and nothing distinguished "the user
/// chose PNG" from "nobody said anything".
///
/// `crates/app/src/output.rs` then re-derives the format from the final path,
/// so honouring the name here is what makes that inference correct rather than
/// dead.
fn with_extension(path: PathBuf, format: SaveFormat) -> PathBuf {
    let current = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);

    match current {
        // A recognised extension is the user's explicit choice; leave it.
        Some(ext) if KNOWN_EXTENSIONS.contains(&ext.as_str()) => {
            let _ = format;
            path
        }
        // No extension, or one that is really part of the name.
        _ => {
            let mut name = path.file_name().unwrap_or_default().to_os_string();
            name.push(".");
            name.push(format.extension());
            path.with_file_name(name)
        }
    }
}

/// Expand a leading `~` (alone or followed by a separator). `~user` is not
/// supported, and a `~` anywhere but the front is an ordinary character.
fn expand_tilde(path: &str) -> String {
    let rest = match path.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') || rest.starts_with('\\') => rest,
        _ => return path.to_owned(),
    };
    let Some(home) = home_dir() else {
        return path.to_owned();
    };
    let Some(home) = home.to_str() else {
        return path.to_owned();
    };
    format!("{}{}", home.trim_end_matches(['/', '\\']), rest)
}

fn home_dir() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf())
}

/// Walk the template, handing only the parts that look like a placeholder to
/// chrono.
///
/// Formatting the whole template in one go would be shorter, but chrono treats
/// a single unrecognised placeholder as an error for the entire string; doing
/// it span by span means one odd `%` costs only itself.
fn expand_strftime(template: &str, now: DateTime<Local>) -> String {
    let bytes = template.as_bytes();
    let mut out = String::with_capacity(template.len());
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] != b'%' {
            let start = i;
            while i < bytes.len() && bytes[i] != b'%' {
                i += 1;
            }
            out.push_str(&template[start..i]);
            continue;
        }

        match placeholder_end(bytes, i) {
            Some(end) => {
                let spec = &template[i..end];
                if spec == "%%" {
                    out.push('%');
                } else if let Some(rendered) = format_spec(spec, now) {
                    out.push_str(&rendered);
                } else {
                    out.push_str(spec);
                }
                i = end;
            }
            None => {
                out.push('%');
                i += 1;
            }
        }
    }
    out
}

/// Index just past the placeholder starting at `start` (which must be a `%`),
/// or `None` if what follows cannot be one.
fn placeholder_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut i = start + 1;
    if i >= bytes.len() {
        return None;
    }
    if bytes[i] == b'%' {
        return Some(i + 1);
    }
    // Padding and case flags, then an optional width.
    while i < bytes.len() && matches!(bytes[i], b'-' | b'_' | b'0' | b'^' | b'#') {
        i += 1;
    }
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    // `%:z`, `%::z`, `%:::z`.
    while i < bytes.len() && bytes[i] == b':' {
        i += 1;
    }
    match bytes.get(i) {
        Some(b) if b.is_ascii_alphabetic() => Some(i + 1),
        _ => None,
    }
}

/// Render a single placeholder, or `None` if chrono rejects it.
fn format_spec(spec: &str, now: DateTime<Local>) -> Option<String> {
    let items: Vec<Item<'_>> = StrftimeItems::new(spec).collect();
    if items.iter().any(|item| matches!(item, Item::Error)) {
        return None;
    }
    let mut rendered = String::new();
    write!(rendered, "{}", now.format_with_items(items.iter())).ok()?;
    Some(rendered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    /// A fixed instant, so these tests say the same thing in every timezone.
    fn at(hour: u32, minute: u32, second: u32) -> DateTime<Local> {
        Local
            .with_ymd_and_hms(2026, 8, 11, hour, minute, second)
            .single()
            .expect("2026-08-11 is not near a DST transition anywhere sensible")
    }

    fn config_with(output: &str, format: SaveFormat) -> Config {
        Config {
            output_filename: Some(output.to_owned()),
            save_format: format,
            ..Config::default()
        }
    }

    #[test]
    fn strftime_placeholders_expand_against_the_given_instant() {
        assert_eq!(
            expand_filename("shot-%Y-%m-%d_%H-%M-%S.png", at(14, 30, 5)),
            "shot-2026-08-11_14-30-05.png"
        );
        assert_eq!(
            expand_filename("%Y%m%d-%H%M%S.png", at(9, 4, 1)),
            "20260811-090401.png"
        );
    }

    #[test]
    fn padding_flags_are_understood() {
        // `%-d` is chrono's "no padding", which a naive scanner would drop.
        assert_eq!(expand_filename("%-d-%-m.png", at(0, 0, 0)), "11-8.png");
    }

    #[test]
    fn a_template_without_placeholders_is_returned_unchanged() {
        assert_eq!(expand_filename("shot.png", at(1, 2, 3)), "shot.png");
        assert_eq!(expand_filename("", at(1, 2, 3)), "");
        assert_eq!(
            expand_filename("/tmp/a b/c.png", at(1, 2, 3)),
            "/tmp/a b/c.png"
        );
    }

    #[test]
    fn a_literal_percent_survives() {
        // `%%` is the escape...
        assert_eq!(expand_filename("100%%.png", at(1, 2, 3)), "100%.png");
        // ...but a stray `%` that cannot start a placeholder is kept as-is
        // rather than failing the whole expansion.
        assert_eq!(expand_filename("100%.png", at(1, 2, 3)), "100%.png");
        assert_eq!(expand_filename("50% done", at(1, 2, 3)), "50% done");
        assert_eq!(expand_filename("trailing%", at(1, 2, 3)), "trailing%");
    }

    #[test]
    fn an_unknown_placeholder_costs_only_itself() {
        let out = expand_filename("%Y-%!-%d.png", at(0, 0, 0));
        assert_eq!(out, "2026-%!-11.png");
    }

    #[test]
    fn a_leading_tilde_becomes_the_home_directory() {
        let Some(home) = home_dir() else {
            return; // No home directory in this environment; nothing to assert.
        };
        let home = home.to_string_lossy().into_owned();
        let expected = format!("{}/shots/2026.png", home.trim_end_matches('/'));
        assert_eq!(expand_filename("~/shots/%Y.png", at(0, 0, 0)), expected);
        assert_eq!(
            expand_filename("~", at(0, 0, 0)),
            home.trim_end_matches('/')
        );
    }

    #[test]
    fn a_tilde_that_is_not_leading_is_just_a_character() {
        assert_eq!(
            expand_filename("/tmp/~x/a.png", at(0, 0, 0)),
            "/tmp/~x/a.png"
        );
        assert_eq!(expand_filename("~user/a.png", at(0, 0, 0)), "~user/a.png");
    }

    #[test]
    fn no_output_filename_means_no_path() {
        assert_eq!(resolve_output_path(&Config::default(), at(0, 0, 0)), None);
    }

    #[test]
    fn a_missing_extension_is_added() {
        let config = config_with("/tmp/shot", SaveFormat::Png);
        assert_eq!(
            resolve_output_path(&config, at(0, 0, 0)),
            Some(PathBuf::from("/tmp/shot.png"))
        );

        let config = config_with("/tmp/%Y-%m-%d", SaveFormat::Webp);
        assert_eq!(
            resolve_output_path(&config, at(0, 0, 0)),
            Some(PathBuf::from("/tmp/2026-08-11.webp"))
        );
    }

    #[test]
    fn a_matching_extension_is_preserved() {
        let config = config_with("/tmp/shot.png", SaveFormat::Png);
        assert_eq!(
            resolve_output_path(&config, at(0, 0, 0)),
            Some(PathBuf::from("/tmp/shot.png"))
        );

        // `.jpeg` and `.jpg` are the same format, so neither gets rewritten.
        let config = config_with("/tmp/shot.jpeg", SaveFormat::Jpeg);
        assert_eq!(
            resolve_output_path(&config, at(0, 0, 0)),
            Some(PathBuf::from("/tmp/shot.jpeg"))
        );

        // Case is not a disagreement either.
        let config = config_with("/tmp/shot.PNG", SaveFormat::Png);
        assert_eq!(
            resolve_output_path(&config, at(0, 0, 0)),
            Some(PathBuf::from("/tmp/shot.PNG"))
        );
    }

    #[test]
    fn an_extension_the_user_typed_wins_over_the_configured_format() {
        // The old behaviour rewrote .jpg to .png here, which meant asking for
        // JPEG by filename silently produced a PNG — and `save_format`'s
        // default *is* Png, so this fired for everyone who never set it.
        let config = config_with("/tmp/shot.jpg", SaveFormat::Png);
        assert_eq!(
            resolve_output_path(&config, at(0, 0, 0)),
            Some(PathBuf::from("/tmp/shot.jpg")),
            "a typed .jpg must survive the default save-format"
        );

        // And it wins in the other direction too.
        let config = config_with("/tmp/shot.png", SaveFormat::Jpeg);
        assert_eq!(
            resolve_output_path(&config, at(0, 0, 0)),
            Some(PathBuf::from("/tmp/shot.png"))
        );

        // With no extension, the configured format supplies one.
        let config = config_with("/tmp/shot", SaveFormat::Jpeg);
        assert_eq!(
            resolve_output_path(&config, at(0, 0, 0)),
            Some(PathBuf::from("/tmp/shot.jpg"))
        );
    }

    #[test]
    fn a_dotted_name_keeps_its_tail_and_gains_an_extension() {
        let config = config_with("/tmp/report.2026.final", SaveFormat::Png);
        assert_eq!(
            resolve_output_path(&config, at(0, 0, 0)),
            Some(PathBuf::from("/tmp/report.2026.final.png")),
            "`.final` is part of the name, not a format claim"
        );
    }

    #[test]
    fn stdout_passes_through_untouched() {
        let config = config_with("-", SaveFormat::Png);
        assert_eq!(
            resolve_output_path(&config, at(0, 0, 0)),
            Some(PathBuf::from("-")),
            "`-` means stdout and must not become `-.png`"
        );
    }

    #[test]
    fn the_template_is_expanded_before_the_extension_is_checked() {
        // Asserted on the components rather than the whole string: a
        // template written with `/` keeps those separators where the path is
        // returned untouched, but `with_file_name` rebuilds the tail with the
        // platform separator, so on Windows the result legitimately mixes the
        // two (`C:\Users\me/shots\shot.webp`). Both are valid there.
        let expect = |template: &str, name: &str| {
            let config = config_with(template, SaveFormat::Webp);
            let path = resolve_output_path(&config, at(14, 30, 5)).expect("configured");
            assert_eq!(
                path.file_name().and_then(|n| n.to_str()),
                Some(name),
                "got {}",
                path.display()
            );
            assert_eq!(
                path.parent()
                    .and_then(|p| p.file_name())
                    .and_then(|n| n.to_str()),
                Some("shots"),
                "got {}",
                path.display()
            );
            assert!(
                !path.to_string_lossy().starts_with('~'),
                "the tilde should be gone: {}",
                path.display()
            );
        };

        // A template that names its own extension keeps it, even with a
        // different save-format configured.
        expect("~/shots/%Y%m%d-%H%M%S.jpg", "20260811-143005.jpg");

        // One that does not gets the configured format, after expansion — the
        // point of the test name: `%S` must not be mistaken for an extension.
        expect("~/shots/%Y%m%d-%H%M%S", "20260811-143005.webp");
    }
}
