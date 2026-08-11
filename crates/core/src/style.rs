//! Colors and drawing style.
//!
//! Adapted from Satty (`src/style.rs`), MPL-2.0, Copyright the Satty authors.

use std::fmt::Display;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self::new(r, g, b, 255)
    }

    // The default palette, carried over from Satty so existing configs and
    // muscle memory transfer.
    pub const fn orange() -> Self {
        Self::rgb(240, 147, 43)
    }
    pub const fn red() -> Self {
        Self::rgb(235, 77, 75)
    }
    pub const fn green() -> Self {
        Self::rgb(106, 176, 76)
    }
    pub const fn blue() -> Self {
        Self::rgb(34, 166, 179)
    }
    pub const fn cove() -> Self {
        Self::rgb(19, 15, 64)
    }
    pub const fn pink() -> Self {
        Self::rgb(200, 37, 184)
    }
    pub const fn white() -> Self {
        Self::rgb(255, 255, 255)
    }
    pub const fn black() -> Self {
        Self::rgb(0, 0, 0)
    }
    pub const fn transparent() -> Self {
        Self::new(0, 0, 0, 0)
    }

    /// RGB channels inverted, alpha preserved.
    pub fn inverted(self) -> Self {
        Self::new(255 - self.r, 255 - self.g, 255 - self.b, self.a)
    }

    pub fn with_alpha(self, a: u8) -> Self {
        Self::new(self.r, self.g, self.b, a)
    }

    /// Multiply alpha by `factor` (0.0..=1.0).
    pub fn scale_alpha(self, factor: f32) -> Self {
        self.with_alpha((self.a as f32 * factor.clamp(0.0, 1.0)).round() as u8)
    }

    /// WCAG relative luminance, 0.0 (black) to 1.0 (white).
    pub fn luminance(self) -> f32 {
        fn linearize(channel: u8) -> f32 {
            let c = channel as f32 / 255.0;
            if c <= 0.03928 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * linearize(self.r) + 0.7152 * linearize(self.g) + 0.0722 * linearize(self.b)
    }

    /// Black or white, whichever contrasts better with this color. Used for
    /// text drawn on top of a filled shape (e.g. numbered markers).
    pub fn contrast(self) -> Self {
        if self.luminance() > 0.179 {
            Self::new(0, 0, 0, self.a)
        } else {
            Self::new(255, 255, 255, self.a)
        }
    }

    pub fn to_rgba_f32(self) -> [f32; 4] {
        [
            self.r as f32 / 255.0,
            self.g as f32 / 255.0,
            self.b as f32 / 255.0,
            self.a as f32 / 255.0,
        ]
    }

    pub fn to_array(self) -> [u8; 4] {
        [self.r, self.g, self.b, self.a]
    }

    /// `#rrggbb` or `#rrggbbaa` (alpha omitted when opaque).
    pub fn to_hex(self) -> String {
        if self.a == 255 {
            format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
        } else {
            format!("#{:02x}{:02x}{:02x}{:02x}", self.r, self.g, self.b, self.a)
        }
    }
}

impl Default for Color {
    fn default() -> Self {
        Self::red()
    }
}

impl Display for Color {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_hex())
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("invalid color `{0}`: expected #rgb, #rgba, #rrggbb or #rrggbbaa")]
pub struct ColorParseError(String);

impl FromStr for Color {
    type Err = ColorParseError;

    /// Accepts `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`; the leading `#` is
    /// optional.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || ColorParseError(s.to_owned());
        let hex = s.trim().trim_start_matches('#');
        if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(err());
        }
        let nibble = |i: usize| -> Result<u8, ColorParseError> {
            u8::from_str_radix(&hex[i..=i], 16).map_err(|_| err())
        };
        let byte = |i: usize| -> Result<u8, ColorParseError> {
            u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| err())
        };
        match hex.len() {
            3 => Ok(Color::new(
                nibble(0)? * 17,
                nibble(1)? * 17,
                nibble(2)? * 17,
                255,
            )),
            4 => Ok(Color::new(
                nibble(0)? * 17,
                nibble(1)? * 17,
                nibble(2)? * 17,
                nibble(3)? * 17,
            )),
            6 => Ok(Color::new(byte(0)?, byte(2)?, byte(4)?, 255)),
            8 => Ok(Color::new(byte(0)?, byte(2)?, byte(4)?, byte(6)?)),
            _ => Err(err()),
        }
    }
}

impl TryFrom<String> for Color {
    type Error = ColorParseError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.parse()
    }
}

impl From<Color> for String {
    fn from(value: Color) -> Self {
        value.to_hex()
    }
}

/// Discrete stroke sizes. Kept as an enum (rather than a raw width) so the
/// toolbar, keybindings and config all agree on the same three steps, matching
/// Satty. The continuous `annotation_size_factor` scales all of them.
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Size {
    Small = 0,
    #[default]
    Medium = 1,
    Large = 2,
}

impl Size {
    pub const ALL: [Size; 3] = [Size::Small, Size::Medium, Size::Large];

    pub fn from_index(i: usize) -> Option<Size> {
        Size::ALL.get(i).copied()
    }

    pub fn next(self) -> Size {
        match self {
            Size::Small => Size::Medium,
            _ => Size::Large,
        }
    }

    pub fn previous(self) -> Size {
        match self {
            Size::Large => Size::Medium,
            _ => Size::Small,
        }
    }

    pub fn to_text_size(self, factor: f32) -> f32 {
        match self {
            Size::Small => 36.0 * factor,
            Size::Medium => 54.0 * factor,
            Size::Large => 96.0 * factor,
        }
    }

    pub fn to_line_width(self, factor: f32) -> f32 {
        match self {
            Size::Small => 3.0 * factor,
            Size::Medium => 5.0 * factor,
            Size::Large => 7.0 * factor,
        }
    }

    pub fn to_arrow_tail_width(self, factor: f32) -> f32 {
        match self {
            Size::Small => 3.0 * factor,
            Size::Medium => 10.0 * factor,
            Size::Large => 25.0 * factor,
        }
    }

    pub fn to_arrow_head_length(self, factor: f32) -> f32 {
        match self {
            Size::Small => 15.0 * factor,
            Size::Medium => 30.0 * factor,
            Size::Large => 60.0 * factor,
        }
    }

    pub fn to_blur_factor(self, factor: f32) -> f32 {
        match self {
            Size::Small => 10.0 * factor,
            Size::Medium => 20.0 * factor,
            Size::Large => 30.0 * factor,
        }
    }

    pub fn to_highlight_width(self, factor: f32) -> f32 {
        match self {
            Size::Small => 15.0 * factor,
            Size::Medium => 30.0 * factor,
            Size::Large => 45.0 * factor,
        }
    }
}

impl Display for Size {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Size::Small => "small",
            Size::Medium => "medium",
            Size::Large => "large",
        };
        f.write_str(name)
    }
}

/// The full style a tool applies to the drawable it produces. Copied into each
/// drawable at commit time, so later toolbar changes never retroactively alter
/// finished annotations.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    pub color: Color,
    pub size: Size,
    /// Shapes are filled rather than outlined.
    pub fill: bool,
    /// Round line caps and joins.
    pub round_caps: bool,
    /// Global multiplier applied to every derived dimension.
    pub annotation_size_factor: f32,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            color: Color::default(),
            size: Size::default(),
            fill: false,
            round_caps: true,
            annotation_size_factor: 1.0,
        }
    }
}

impl Style {
    pub fn line_width(&self) -> f32 {
        self.size.to_line_width(self.annotation_size_factor)
    }
    pub fn text_size(&self) -> f32 {
        self.size.to_text_size(self.annotation_size_factor)
    }
    pub fn arrow_tail_width(&self) -> f32 {
        self.size.to_arrow_tail_width(self.annotation_size_factor)
    }
    pub fn arrow_head_length(&self) -> f32 {
        self.size.to_arrow_head_length(self.annotation_size_factor)
    }
    pub fn blur_factor(&self) -> f32 {
        self.size.to_blur_factor(self.annotation_size_factor)
    }
    pub fn highlight_width(&self) -> f32 {
        self.size.to_highlight_width(self.annotation_size_factor)
    }

    pub fn with_color(mut self, color: Color) -> Self {
        self.color = color;
        self
    }
    pub fn with_size(mut self, size: Size) -> Self {
        self.size = size;
        self
    }
    pub fn with_fill(mut self, fill: bool) -> Self {
        self.fill = fill;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_hex_forms() {
        assert_eq!("#ff0000".parse::<Color>().unwrap(), Color::rgb(255, 0, 0));
        assert_eq!("ff0000".parse::<Color>().unwrap(), Color::rgb(255, 0, 0));
        assert_eq!("#f00".parse::<Color>().unwrap(), Color::rgb(255, 0, 0));
        assert_eq!(
            "#ff000080".parse::<Color>().unwrap(),
            Color::new(255, 0, 0, 128)
        );
        assert_eq!(
            "#f00f".parse::<Color>().unwrap(),
            Color::new(255, 0, 0, 255)
        );
    }

    #[test]
    fn rejects_malformed_hex() {
        for bad in ["#gg0000", "#12345", "", "#", "rgb(1,2,3)"] {
            assert!(bad.parse::<Color>().is_err(), "{bad} should not parse");
        }
    }

    #[test]
    fn hex_roundtrips() {
        for c in [
            Color::red(),
            Color::new(1, 2, 3, 4),
            Color::rgb(0, 0, 0),
            Color::orange(),
        ] {
            assert_eq!(c.to_hex().parse::<Color>().unwrap(), c);
        }
    }

    #[test]
    fn opaque_hex_omits_alpha() {
        assert_eq!(Color::rgb(18, 52, 86).to_hex(), "#123456");
        assert_eq!(Color::new(18, 52, 86, 128).to_hex(), "#12345680");
    }

    #[test]
    fn contrast_picks_readable_foreground() {
        assert_eq!(Color::white().contrast(), Color::black());
        assert_eq!(Color::black().contrast(), Color::white());
        // The default palette colours are mid-tone; just assert we get one of
        // the two extremes and that alpha survives.
        let c = Color::new(240, 147, 43, 200).contrast();
        assert_eq!(c.a, 200);
        assert!(c == Color::black().with_alpha(200) || c == Color::white().with_alpha(200));
    }

    #[test]
    fn sizes_scale_with_the_factor() {
        assert_eq!(Size::Medium.to_line_width(1.0), 5.0);
        assert_eq!(Size::Medium.to_line_width(2.0), 10.0);
        assert!(Size::Small.to_line_width(1.0) < Size::Large.to_line_width(1.0));
    }

    #[test]
    fn size_steps_saturate_at_the_ends() {
        assert_eq!(Size::Small.previous(), Size::Small);
        assert_eq!(Size::Large.next(), Size::Large);
        assert_eq!(Size::Small.next(), Size::Medium);
    }

    #[test]
    fn style_helpers_apply_the_factor() {
        let s = Style {
            size: Size::Medium,
            annotation_size_factor: 2.0,
            ..Default::default()
        };
        assert_eq!(s.line_width(), 10.0);
        assert_eq!(s.text_size(), 108.0);
    }
}
