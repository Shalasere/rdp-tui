//! Data-only, validated theme configuration; independent of the TUI renderer.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt;

/// Semantic styling roles understood by the application.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ThemeRole {
    Text,
    TextMuted,
    Title,
    Border,
    BorderFocused,
    Selection,
    FieldFocused,
    Info,
    Success,
    Warning,
    Error,
}

impl ThemeRole {
    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "text" => Self::Text,
            "text_muted" => Self::TextMuted,
            "title" => Self::Title,
            "border" => Self::Border,
            "border_focused" => Self::BorderFocused,
            "selection" => Self::Selection,
            "field_focused" => Self::FieldFocused,
            "info" => Self::Info,
            "success" => Self::Success,
            "warning" => Self::Warning,
            "error" => Self::Error,
            _ => return None,
        })
    }
}

/// A terminal default, ANSI palette entry, or exact RGB color.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThemeColor {
    Default,
    Ansi(u8),
    Rgb(u8, u8, u8),
}

/// Supported display modifiers. Hidden and blinking text are deliberately absent.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ThemeModifier {
    Bold,
    Dim,
    Italic,
    Underline,
    Reversed,
}

/// A partial override. `None` inherits, while `Some(Default)` resets a color.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ThemeStyle {
    pub fg: Option<ThemeColor>,
    pub bg: Option<ThemeColor>,
    pub modifiers: Option<Vec<ThemeModifier>>,
}

/// A fully validated theme; all palette references have been resolved.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ThemeDocument {
    pub styles: BTreeMap<ThemeRole, ThemeStyle>,
}

/// A concise diagnostic without dumping the input document.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThemeError {
    pub path: String,
    pub message: String,
}

impl fmt::Display for ThemeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.path, self.message)
    }
}

impl std::error::Error for ThemeError {}

fn invalid(path: impl Into<String>, message: &str) -> ThemeError {
    ThemeError {
        path: path.into(),
        message: message.to_string(),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDocument {
    version: u32,
    #[serde(default)]
    palette: BTreeMap<String, String>,
    #[serde(default)]
    styles: BTreeMap<String, RawStyle>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStyle {
    fg: Option<String>,
    bg: Option<String>,
    modifiers: Option<Vec<ThemeModifier>>,
}

fn literal_color(value: &str) -> Option<ThemeColor> {
    if value == "default" {
        return Some(ThemeColor::Default);
    }
    let names = [
        "black",
        "red",
        "green",
        "yellow",
        "blue",
        "magenta",
        "cyan",
        "gray",
        "dark_gray",
        "light_red",
        "light_green",
        "light_yellow",
        "light_blue",
        "light_magenta",
        "light_cyan",
        "white",
    ];
    if let Some(index) = names.iter().position(|name| *name == value) {
        return u8::try_from(index).ok().map(ThemeColor::Ansi);
    }
    let hex = value.strip_prefix('#')?;
    if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    Some(ThemeColor::Rgb(
        u8::from_str_radix(&hex[..2], 16).ok()?,
        u8::from_str_radix(&hex[2..4], 16).ok()?,
        u8::from_str_radix(&hex[4..], 16).ok()?,
    ))
}

fn resolve_color(
    value: &str,
    palette: &BTreeMap<String, ThemeColor>,
    path: &str,
) -> Result<ThemeColor, ThemeError> {
    if let Some(name) = value.strip_prefix('$') {
        return palette
            .get(name)
            .copied()
            .ok_or_else(|| invalid(path, "undefined palette reference"));
    }
    literal_color(value)
        .ok_or_else(|| invalid(path, "expected default, ANSI color name, or #RRGGBB"))
}

/// Parse a version-1 theme and resolve its literal palette references.
///
/// # Errors
/// Returns a location-bearing error for malformed TOML, unknown fields or roles,
/// unsupported versions/modifiers, or invalid colors and palette references.
pub fn parse_theme_document(text: &str) -> Result<ThemeDocument, ThemeError> {
    let raw: RawDocument = toml::from_str(text).map_err(|error| {
        let path = error.span().map_or_else(
            || "document".to_string(),
            |span| format!("bytes {}..{}", span.start, span.end),
        );
        invalid(
            path,
            "invalid theme TOML or unsupported field/type/modifier; check the theme format",
        )
    })?;
    if raw.version != 1 {
        return Err(invalid("version", "expected theme version 1"));
    }
    let palette = raw
        .palette
        .iter()
        .map(|(name, value)| {
            let color = literal_color(value).ok_or_else(|| {
                invalid(
                    format!("palette.{name}"),
                    "expected a literal color, not an alias",
                )
            })?;
            Ok((name.clone(), color))
        })
        .collect::<Result<BTreeMap<_, _>, ThemeError>>()?;
    let mut styles = BTreeMap::new();
    for (name, style) in raw.styles {
        let role = ThemeRole::parse(&name)
            .ok_or_else(|| invalid(format!("styles.{name}"), "unknown semantic style role"))?;
        let color = |value: Option<String>, property: &str| {
            value
                .map(|value| resolve_color(&value, &palette, &format!("styles.{name}.{property}")))
                .transpose()
        };
        styles.insert(
            role,
            ThemeStyle {
                fg: color(style.fg, "fg")?,
                bg: color(style.bg, "bg")?,
                modifiers: style.modifiers,
            },
        );
    }
    Ok(ThemeDocument { styles })
}
