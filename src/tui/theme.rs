//! Resolve user theme data into Ratatui styles; no colored palettes are compiled in.

use crate::config::theme::{
    ThemeColor, ThemeDocument, ThemeModifier, ThemeRole, ThemeStyle, parse_theme_document,
};
use ratatui::style::{Color, Modifier, Style};
use std::path::Path;

/// Resolved semantic styles, shared by all views.
#[derive(Clone, Debug)]
pub struct Theme {
    pub text: Style,
    pub text_muted: Style,
    pub title: Style,
    pub border: Style,
    pub border_focused: Style,
    pub selection: Style,
    pub field_focused: Style,
    pub info: Style,
    pub success: Style,
    pub warning: Style,
    pub error: Style,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            text: Style::default(),
            text_muted: Style::default(),
            title: Style::default(),
            border: Style::default(),
            border_focused: Style::default(),
            selection: Style::default()
                .fg(Color::Reset)
                .bg(Color::Reset)
                .reversed()
                .remove_modifier(Modifier::DIM),
            field_focused: Style::default()
                .fg(Color::Reset)
                .bg(Color::Reset)
                .reversed()
                .remove_modifier(Modifier::DIM),
            info: Style::default(),
            success: Style::default(),
            warning: Style::default().bold(),
            error: Style::default().bold(),
        }
    }
}

impl Theme {
    /// Load only `theme.toml` from the selected application configuration root.
    /// Missing files retain monochrome; invalid/unreadable files return a warning.
    #[must_use]
    pub fn load(config_root: &Path) -> (Self, Option<String>) {
        let path = config_root.join("theme.toml");
        let problem = match std::fs::read_to_string(&path) {
            Ok(text) => match parse_theme_document(&text) {
                Ok(document) => return (Self::from_document(&document), None),
                Err(error) => error.to_string(),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return (Self::default(), None);
            }
            Err(error) => error.to_string(),
        };
        (
            Self::default(),
            Some(format!(
                "Theme configuration warning: {}: {problem}. Using monochrome. Fix or remove theme.toml and restart rdp-tui.",
                path.display()
            )),
        )
    }

    /// Resolve validated data over the minimal monochrome fallback.
    #[must_use]
    pub fn from_document(document: &ThemeDocument) -> Self {
        let mut theme = Self::default();
        // Resolve the base first, so explicit selection colors can inherit its pair.
        if let Some(style) = document.styles.get(&ThemeRole::Text) {
            theme.text = resolve_style(theme.text, style);
        }
        for (role, definition) in &document.styles {
            let base = theme.text;
            let target = match role {
                ThemeRole::Text => continue,
                ThemeRole::TextMuted => &mut theme.text_muted,
                ThemeRole::Title => &mut theme.title,
                ThemeRole::Border => &mut theme.border,
                ThemeRole::BorderFocused => &mut theme.border_focused,
                ThemeRole::Selection => &mut theme.selection,
                ThemeRole::FieldFocused => &mut theme.field_focused,
                ThemeRole::Info => &mut theme.info,
                ThemeRole::Success => &mut theme.success,
                ThemeRole::Warning => &mut theme.warning,
                ThemeRole::Error => &mut theme.error,
            };
            if matches!(role, ThemeRole::Selection | ThemeRole::FieldFocused)
                && (definition.fg.is_some() || definition.bg.is_some())
            {
                // Interaction wins over content styling. Reset inherited modifiers
                // before applying explicit colors and any user-requested modifiers.
                *target = Style::default()
                    .fg(base.fg.unwrap_or(Color::Reset))
                    .bg(base.bg.unwrap_or(Color::Reset))
                    .remove_modifier(Modifier::REVERSED | Modifier::DIM);
            }
            *target = resolve_style(*target, definition);
        }
        // Reverse the base pair, never a content-specific severity or muted color.
        for (role, style) in [
            (ThemeRole::Selection, &mut theme.selection),
            (ThemeRole::FieldFocused, &mut theme.field_focused),
        ] {
            if style.add_modifier.contains(Modifier::REVERSED) {
                let definition = document.styles.get(&role);
                style.fg = Some(
                    definition
                        .and_then(|value| value.fg)
                        .map(color)
                        .or(theme.text.fg)
                        .unwrap_or(Color::Reset),
                );
                style.bg = Some(
                    definition
                        .and_then(|value| value.bg)
                        .map(color)
                        .or(theme.text.bg)
                        .unwrap_or(Color::Reset),
                );
                let explicit_pair =
                    definition.is_some_and(|value| value.fg.is_some() || value.bg.is_some());
                if !explicit_pair && theme.text.add_modifier.contains(Modifier::REVERSED) {
                    *style = style.remove_modifier(Modifier::REVERSED);
                }
                if !style.add_modifier.contains(Modifier::DIM) {
                    *style = style.remove_modifier(Modifier::DIM);
                }
            }
        }
        theme
    }
}

fn resolve_style(mut style: Style, definition: &ThemeStyle) -> Style {
    if let Some(fg) = definition.fg {
        style = style.fg(color(fg));
    }
    if let Some(bg) = definition.bg {
        style = style.bg(color(bg));
    }
    if let Some(modifiers) = &definition.modifiers {
        style = style.remove_modifier(
            Modifier::BOLD
                | Modifier::DIM
                | Modifier::ITALIC
                | Modifier::UNDERLINED
                | Modifier::REVERSED,
        );
        for modifier in modifiers {
            style = style.add_modifier(match modifier {
                ThemeModifier::Bold => Modifier::BOLD,
                ThemeModifier::Dim => Modifier::DIM,
                ThemeModifier::Italic => Modifier::ITALIC,
                ThemeModifier::Underline => Modifier::UNDERLINED,
                ThemeModifier::Reversed => Modifier::REVERSED,
            });
        }
    }
    style
}

fn color(value: ThemeColor) -> Color {
    match value {
        ThemeColor::Default => Color::Reset,
        ThemeColor::Rgb(red, green, blue) => Color::Rgb(red, green, blue),
        ThemeColor::Ansi(index) => [
            Color::Black,
            Color::Red,
            Color::Green,
            Color::Yellow,
            Color::Blue,
            Color::Magenta,
            Color::Cyan,
            Color::Gray,
            Color::DarkGray,
            Color::LightRed,
            Color::LightGreen,
            Color::LightYellow,
            Color::LightBlue,
            Color::LightMagenta,
            Color::LightCyan,
            Color::White,
        ]
        .get(usize::from(index))
        .copied()
        .unwrap_or(Color::Reset),
    }
}
