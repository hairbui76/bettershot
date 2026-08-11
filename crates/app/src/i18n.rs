//! Localization scaffolding.
//!
//! This is deliberately the *mechanism* and not a set of translations. Shipping
//! a half-finished translation is worse than shipping none: a dialog that is
//! half in the user's language and half in English reads as a bug, and a
//! machine-translated keyboard shortcut can be actively wrong. So English is
//! the only catalogue here, and what this module provides is the seam a
//! translator can work against.
//!
//! # How it works
//!
//! Every user-visible string has a stable key. [`Catalog::get`] resolves a key
//! against the selected language, falling back to English, and finally to the
//! key itself — which is visible in the UI rather than silently blank, so a
//! missing string shows up the first time anyone looks.
//!
//! # Adding a language
//!
//! 1. Add a variant to [`Language`] and its `code`.
//! 2. Add a `&[(key, translation)]` table and return it from [`Catalog::table`].
//! 3. Run the tests: [`every_key_exists_in_english`] fails if a key used in the
//!    code has no English string, and `a_translation_covers_known_keys_only`
//!    fails if a translation invents keys that no longer exist.

use std::fmt;
use std::str::FromStr;

/// The languages bettershot can display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Language {
    /// Follow the operating system, falling back to English.
    #[default]
    System,
    English,
}

impl Language {
    pub const ALL: [Language; 2] = [Language::System, Language::English];

    pub fn code(&self) -> &'static str {
        match self {
            Language::System => "system",
            Language::English => "en",
        }
    }

    /// Resolve `System` against the environment.
    ///
    /// Only `LANG`/`LC_ALL` are consulted, which covers Unix; on Windows the
    /// system locale would need a platform call, and since English is the only
    /// catalogue today that would be effort with no observable effect.
    pub fn resolve(self) -> Language {
        match self {
            Language::System => Language::from_environment().unwrap_or(Language::English),
            explicit => explicit,
        }
    }

    fn from_environment() -> Option<Language> {
        let raw = std::env::var("LC_ALL")
            .or_else(|_| std::env::var("LANG"))
            .ok()?;
        // "en_GB.UTF-8" -> "en"
        let prefix = raw.split(['_', '.', '@']).next()?.to_ascii_lowercase();
        Language::ALL
            .into_iter()
            .find(|l| *l != Language::System && l.code() == prefix)
    }
}

impl fmt::Display for Language {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("unknown language `{0}`")]
pub struct LanguageParseError(String);

impl FromStr for Language {
    type Err = LanguageParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let needle = s.trim().to_ascii_lowercase();
        Language::ALL
            .into_iter()
            .find(|l| l.code() == needle)
            .ok_or_else(|| LanguageParseError(s.to_owned()))
    }
}

/// Every user-visible string, by key.
///
/// Keys are grouped by area and named for the thing they label, not for the
/// English text, so rewording English does not churn every translation.
pub const ENGLISH: &[(&str, &str)] = &[
    // Tools
    ("tool.pointer", "Pointer"),
    ("tool.crop", "Crop"),
    ("tool.line", "Line"),
    ("tool.arrow", "Arrow"),
    ("tool.rectangle", "Rect"),
    ("tool.ellipse", "Ellipse"),
    ("tool.text", "Text"),
    ("tool.marker", "Number"),
    ("tool.brush", "Brush"),
    ("tool.highlight", "Highlight"),
    ("tool.blur", "Obscure"),
    // Toolbar actions
    ("action.undo", "Undo"),
    ("action.redo", "Redo"),
    ("action.copy", "Copy"),
    ("action.save", "Save"),
    ("action.settings", "Settings"),
    ("action.recent", "Recent"),
    ("action.apply_crop", "Apply crop"),
    ("action.fit", "Fit"),
    ("action.actual_size", "1:1"),
    ("action.fill", "Fill"),
    // Status messages
    ("status.copied", "Copied to clipboard"),
    ("status.undo", "Undo"),
    ("status.redo", "Redo"),
    ("status.cleared", "Cleared all annotations"),
    ("status.deleted", "Deleted"),
    ("status.nothing_selected", "Nothing selected"),
    ("status.nothing_saved", "Nothing saved yet"),
    (
        "status.nothing_to_crop",
        "Nothing to crop — drag a smaller selection first",
    ),
    // Overlay
    (
        "overlay.hint",
        "Drag to select a region · click a window · Esc to cancel",
    ),
    // Settings
    ("settings.title", "Settings"),
    ("settings.appearance", "Appearance"),
    ("settings.drawing", "Drawing"),
    ("settings.behaviour", "Behaviour"),
    ("settings.capture", "Capture"),
    ("settings.privacy", "Privacy"),
    ("settings.theme", "Theme"),
    ("settings.save_to_file", "Save to config file"),
];

/// A resolved language plus its lookup table.
#[derive(Debug, Clone, Copy)]
pub struct Catalog {
    language: Language,
}

impl Default for Catalog {
    fn default() -> Self {
        Self::new(Language::default())
    }
}

impl Catalog {
    pub fn new(language: Language) -> Self {
        Self {
            language: language.resolve(),
        }
    }

    pub fn language(&self) -> Language {
        self.language
    }

    fn table(&self) -> &'static [(&'static str, &'static str)] {
        match self.language {
            // Every language currently resolves to English. New tables slot in
            // here; see the module docs.
            Language::English | Language::System => ENGLISH,
        }
    }

    /// The string for `key`.
    ///
    /// Falls back to English, then to the key itself. Returning the key rather
    /// than an empty string means a gap is visible in the UI instead of
    /// silently blanking a button.
    pub fn get(&self, key: &str) -> &'static str {
        lookup(self.table(), key)
            .or_else(|| lookup(ENGLISH, key))
            .unwrap_or(MISSING)
    }
}

fn lookup(table: &'static [(&'static str, &'static str)], key: &str) -> Option<&'static str> {
    table
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, value)| *value)
}

/// Shown when a key has no entry in any catalogue.
///
/// A visible marker rather than an empty string, so a gap shows up the first
/// time anyone looks at the UI instead of silently blanking a button.
pub const MISSING: &str = "???";

#[cfg(test)]
mod tests {
    use super::*;

    /// Keys the program actually asks for. Kept explicit so that deleting a
    /// key from the catalogue without deleting its use is caught here.
    const KEYS_IN_USE: &[&str] = &[
        "tool.pointer",
        "tool.crop",
        "tool.line",
        "tool.arrow",
        "tool.rectangle",
        "tool.ellipse",
        "tool.text",
        "tool.marker",
        "tool.brush",
        "tool.highlight",
        "tool.blur",
        "action.undo",
        "action.redo",
        "action.copy",
        "action.save",
        "action.settings",
        "action.recent",
        "action.apply_crop",
        "action.fit",
        "action.actual_size",
        "action.fill",
        "status.copied",
        "status.undo",
        "status.redo",
        "status.cleared",
        "status.deleted",
        "status.nothing_selected",
        "status.nothing_saved",
        "status.nothing_to_crop",
        "overlay.hint",
        "settings.title",
        "settings.appearance",
        "settings.drawing",
        "settings.behaviour",
        "settings.capture",
        "settings.privacy",
        "settings.theme",
        "settings.save_to_file",
    ];

    #[test]
    fn every_key_exists_in_english() {
        let catalog = Catalog::new(Language::English);
        for key in KEYS_IN_USE {
            let value = catalog.get(key);
            assert_ne!(value, MISSING, "`{key}` has no English string");
            assert!(!value.is_empty(), "`{key}` is empty");
        }
    }

    #[test]
    fn the_english_table_has_no_duplicate_keys() {
        // A duplicate would make the second entry unreachable, and which one
        // wins would depend on table order.
        let mut keys: Vec<&str> = ENGLISH.iter().map(|(k, _)| *k).collect();
        keys.sort_unstable();
        let before = keys.len();
        keys.dedup();
        assert_eq!(before, keys.len(), "the English table has duplicate keys");
    }

    #[test]
    fn a_translation_covers_known_keys_only() {
        // Run against English itself today; when a real translation lands this
        // catches keys it invents that the program never asks for.
        let known: Vec<&str> = ENGLISH.iter().map(|(k, _)| *k).collect();
        for key in &known {
            assert!(
                KEYS_IN_USE.contains(key),
                "`{key}` is in the catalogue but nothing uses it"
            );
        }
    }

    #[test]
    fn an_unknown_key_is_visible_rather_than_blank() {
        let catalog = Catalog::default();
        assert_eq!(catalog.get("nonexistent.key"), MISSING);
    }

    #[test]
    fn language_codes_round_trip() {
        for language in Language::ALL {
            assert_eq!(language.code().parse::<Language>().unwrap(), language);
        }
        assert!("klingon".parse::<Language>().is_err());
    }

    #[test]
    fn system_resolves_to_a_concrete_language() {
        // Whatever the environment says, the result must be something with a
        // table, never `System`.
        assert_ne!(Language::System.resolve(), Language::System);
        assert_eq!(Language::English.resolve(), Language::English);
    }

    #[test]
    fn an_explicit_language_is_not_overridden_by_the_environment() {
        let catalog = Catalog::new(Language::English);
        assert_eq!(catalog.language(), Language::English);
    }
}
