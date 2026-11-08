//! Pure rendering for the TUI, separated from controller and state transitions.

use super::{App, FIELDS, Mode, PasswordPurpose, Severity, TextView};
use crate::tui::theme::Theme;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap};

pub(super) fn draw(app: &mut App, frame: &mut ratatui::Frame) {
    let theme = &app.theme;
    frame.render_widget(Block::new().style(theme.text), frame.area());
    let footer = footer(app, frame.area().width);
    let footer_height = u16::try_from(footer.line_count(frame.area().width.saturating_sub(2)))
        .unwrap_or(u16::MAX)
        .saturating_add(2)
        .min(frame.area().height.saturating_sub(3));
    let footer = footer.block(panel("", theme, theme.border));
    let areas = Layout::vertical([Constraint::Min(3), Constraint::Length(footer_height)])
        .split(frame.area());

    if let Mode::Editing(form) = &app.mode {
        let list = editing_list(form, theme);
        let mut state = ListState::default();
        state.select(Some(form.field));
        frame.render_stateful_widget(list, areas[0], &mut state);
    } else if let Mode::Status(details) = &mut app.mode {
        let block = panel(format!(" {} ", details.title), theme, theme.border_focused);
        let inner = block.inner(areas[0]);
        update_scroll(details, inner);
        frame.render_widget(block, areas[0]);
        frame.render_widget(
            Paragraph::new(details.message.as_str())
                .style(theme.text)
                .wrap(Wrap { trim: false })
                .scroll((details.scroll, 0)),
            inner,
        );
    } else if let Mode::CertificateMismatch(mismatch) = &app.mode {
        let text = format!(
            "Changed certificate for {}\n\nPinned SHA-256:\n{}\n\nPresented SHA-256:\n{}\n\nVerify this fingerprint through a trusted channel. Press T only if it matches.",
            mismatch.endpoint,
            mismatch.pinned_sha256.as_deref().unwrap_or("unavailable"),
            mismatch.presented_sha256,
        );
        frame.render_widget(
            Paragraph::new(text)
                .style(theme.text)
                .wrap(Wrap { trim: false })
                .block(
                    panel(" certificate warning ", theme, theme.warning).title_style(theme.warning),
                ),
            areas[0],
        );
    } else {
        let items: Vec<ListItem> = app
            .visible
            .iter()
            .enumerate()
            .map(|(visible_index, &index)| {
                let profile = &app.profiles[index];
                // Selection reverses the base text pair, not the endpoint accent.
                let endpoint_style = if app.selected.selected() == Some(visible_index) {
                    theme.text
                } else {
                    theme.text_muted
                };
                ListItem::new(Line::from(vec![
                    Span::raw(profile.name.clone()),
                    Span::styled(format!("    {}", profile.endpoint), endpoint_style),
                ]))
            })
            .collect();
        let list = List::new(items)
            .style(theme.text)
            .block(panel(
                " rdp-tui — profiles ",
                theme,
                if matches!(app.mode, Mode::Browsing) {
                    theme.border_focused
                } else {
                    theme.border
                },
            ))
            .highlight_symbol("> ")
            .highlight_style(theme.selection);
        frame.render_stateful_widget(list, areas[0], &mut app.selected);
    }

    frame.render_widget(footer, areas[1]);
    if matches!(app.mode, Mode::Browsing)
        && let Some(dialog) = app.theme_warning.as_mut()
    {
        draw_message_dialog(dialog, frame, theme, theme.warning);
    }
    if let Some(dialog) = app.error_dialog.as_mut() {
        draw_message_dialog(dialog, frame, theme, theme.error);
    }
}

fn editing_list(form: &super::EditForm, theme: &Theme) -> List<'static> {
    let items: Vec<ListItem> = FIELDS
        .iter()
        .enumerate()
        .map(|(index, &field)| {
            let value = if index == form.field && form.editing_text.is_some() {
                format!("{}_", form.editing_text.as_deref().unwrap_or(""))
            } else {
                form.display_value(field)
            };
            ListItem::new(Line::from(format!("{:<14} {value}", field.label())))
        })
        .collect();
    let title = if form.target.is_some() {
        " edit profile "
    } else {
        " add profile "
    };
    List::new(items)
        .style(theme.text)
        .block(panel(title, theme, theme.border_focused))
        .highlight_symbol("> ")
        .highlight_style(if form.editing_text.is_some() {
            theme.field_focused
        } else {
            theme.selection
        })
}

fn footer(app: &App, width: u16) -> Paragraph<'static> {
    let (title, body) = match &app.mode {
        Mode::Browsing => (
            " Enter connect · a/e add/edit · c clone · d delete · f find · i/x import/export · s status · h history · m messages · t test · D deep-test · p/g pass · T cert · ? help · q quit ".to_string(),
            app.status.clone(),
        ),
        Mode::Password { input, purpose } => (
            match purpose {
                PasswordPurpose::Save(_) => " typing password · Enter save · Esc cancel ",
                PasswordPurpose::Connect(_) => " typing password · Enter connect · Esc cancel ",
            }
            .to_string(),
            format!("Password: {}", "*".repeat(input.chars().count())),
        ),
        Mode::Prompt { label, input, .. } => (
            " Enter confirm · Esc cancel ".to_string(),
            format!("{label}: {input}"),
        ),
        Mode::Status(_) => (
            if width < 60 { " ↑↓ scroll · Enter/Esc close " } else { " ↑↓ scroll · PgUp/PgDn page · Home/End · Enter/Esc close " }.to_string(),
            String::new(),
        ),
        Mode::CertificateMismatch(_) => (
            " T trust exact fingerprint · Q/Esc cancel ".to_string(),
            String::new(),
        ),
        Mode::Editing(form) => (
            if form.editing_text.is_some() {
                " typing · Enter set · Esc cancel field ".to_string()
            } else {
                " ↑/↓ field · Enter/Space edit · A accept · Esc cancel ".to_string()
            },
            form.error.clone().unwrap_or_else(|| app.status.clone()),
        ),
    };
    let theme = &app.theme;
    let mut lines = Vec::new();
    if app.notification_pending {
        let hint = if matches!(app.mode, Mode::Browsing) {
            "Error · m details"
        } else {
            "Error details pending"
        };
        lines.push(Line::styled(hint, theme.error));
    }
    lines.push(Line::styled(title.trim().to_string(), theme.text_muted));
    if !body.is_empty() {
        let style = match &app.mode {
            Mode::Prompt {
                action:
                    super::PromptAction::ConfirmDelete(_) | super::PromptAction::ConfirmDeepTest(_),
                ..
            } => theme.warning,
            Mode::Password { .. } | Mode::Prompt { .. } => theme.field_focused,
            Mode::Editing(form) if form.error.is_some() => theme.error,
            _ => severity_style(theme, app.status_severity),
        };
        // Split embedded newlines without changing the footer's wrapping or height.
        lines.extend(
            body.split('\n')
                .map(|line| Line::styled(line.to_string(), style)),
        );
    }
    Paragraph::new(Text::from(lines))
        .style(theme.text)
        .wrap(Wrap { trim: true })
}

fn severity_style(theme: &Theme, severity: Severity) -> Style {
    match severity {
        Severity::Neutral => theme.text,
        Severity::Info => theme.info,
        Severity::Success => theme.success,
        Severity::Warning => theme.warning,
        Severity::Error => theme.error,
    }
}

fn panel<'a>(title: impl Into<Line<'a>>, theme: &Theme, border: Style) -> Block<'a> {
    Block::bordered()
        .style(theme.text)
        .border_style(border)
        .title(title)
        .title_style(theme.title)
}

fn draw_message_dialog(
    dialog: &mut TextView,
    frame: &mut ratatui::Frame,
    theme: &Theme,
    accent: Style,
) {
    let area = frame.area();
    let width = area.width.saturating_sub(4).max(20).min(area.width);
    let inner_width = width.saturating_sub(2);
    let content_height = u16::try_from(
        Paragraph::new(dialog.message.as_str())
            .style(theme.text)
            .wrap(Wrap { trim: false })
            .line_count(inner_width),
    )
    .unwrap_or(u16::MAX);
    let instructions = if inner_width < 60 {
        "↑↓ scroll · Enter/Esc close"
    } else {
        "↑↓ scroll · PgUp/PgDn page · Home/End · Enter/Esc close"
    };
    let hints = Paragraph::new(instructions)
        .style(theme.text_muted)
        .wrap(Wrap { trim: true });
    let hints_height = u16::try_from(hints.line_count(inner_width)).unwrap_or(u16::MAX);
    let height = content_height
        .saturating_add(hints_height)
        .saturating_add(3)
        .min(area.height.saturating_sub(2).max(6).min(area.height));
    let popup = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    let block = panel(format!(" {} ", dialog.title), theme, accent).title_style(accent);
    let inner = block.inner(popup);
    let hints_height = hints_height.min(inner.height.saturating_sub(1));
    let sections =
        Layout::vertical([Constraint::Min(1), Constraint::Length(hints_height)]).split(inner);
    update_scroll(dialog, sections[0]);
    let block = if dialog.max_scroll > 0 {
        block.title_bottom(format!(
            " {}/{} ",
            dialog.scroll.saturating_add(1),
            content_height
        ))
    } else {
        block
    };
    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);
    frame.render_widget(
        Paragraph::new(dialog.message.as_str())
            .style(theme.text)
            .wrap(Wrap { trim: false })
            .scroll((dialog.scroll, 0)),
        sections[0],
    );
    frame.render_widget(hints, sections[1]);
}

fn update_scroll(details: &mut TextView, area: Rect) {
    let lines = Paragraph::new(details.message.as_str())
        .wrap(Wrap { trim: false })
        .line_count(area.width);
    details.page_height = area.height.max(1);
    details.max_scroll = u16::try_from(lines)
        .unwrap_or(u16::MAX)
        .saturating_sub(details.page_height);
    details.scroll = details.scroll.min(details.max_scroll);
}

#[cfg(test)]
mod tests {
    use super::{App, Mode, Severity, TextView, draw};
    use crate::model::{
        DeviceConfig, DisplayConfig, IdentityConfig, Profile, ProfileId, Route, SecurityConfig,
    };
    use crate::operations::CredentialSlot;
    use crate::tui::theme::Theme;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::style::{Color, Modifier};
    use std::path::PathBuf;

    fn app() -> App {
        let profile = Profile {
            id: ProfileId::generate(),
            name: "Synthetic profile".into(),
            endpoint: "example.invalid:3389".parse().unwrap(),
            identity: IdentityConfig::default(),
            route: Route::Direct,
            display: DisplayConfig::default(),
            devices: DeviceConfig::default(),
            security: SecurityConfig::default(),
            credential: None,
        };
        App::new(
            vec![profile],
            PathBuf::from("rdp-tui"),
            PathBuf::from("/unused-test-config"),
        )
    }

    fn render(app: &mut App, width: u16, height: u16) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(app, frame)).unwrap();
        terminal.backend().buffer().clone()
    }

    fn theme(text: &str) -> Theme {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("theme.toml"), text).unwrap();
        let (theme, warning) = Theme::load(directory.path());
        assert!(warning.is_none(), "{warning:?}");
        theme
    }

    const TEST_THEME: &str = r##"
version = 1
[styles.text]
fg = "#dddddd"
bg = "#111111"
modifiers = ["dim"]
[styles.text_muted]
fg = "#aaaaaa"
[styles.selection]
fg = "#111111"
bg = "#eeeeee"
modifiers = []
[styles.border_focused]
fg = "#55ff55"
[styles.error]
fg = "#ff5555"
[styles.warning]
fg = "#ffff55"
[styles.success]
fg = "#55ffaa"
[styles.info]
fg = "#55aaff"
[styles.field_focused]
fg = "#111111"
bg = "#aaffff"
modifiers = ["bold"]
"##;

    fn cells_for<'a>(buffer: &'a Buffer, needle: &str) -> &'a [ratatui::buffer::Cell] {
        let wanted: Vec<String> = needle.chars().map(|c| c.to_string()).collect();
        buffer
            .content
            .windows(wanted.len())
            .find(|cells| {
                cells
                    .iter()
                    .zip(&wanted)
                    .all(|(cell, symbol)| cell.symbol() == symbol)
            })
            .unwrap_or_else(|| panic!("text {needle:?} absent from rendered buffer"))
    }

    #[test]
    fn runtime_theme_styles_selection_and_focus_without_leaking_muted_or_dim_styles() {
        let mut app = app();
        app.theme = theme(TEST_THEME);
        let buffer = render(&mut app, 80, 24);
        for needle in ["Synthetic profile", "example.invalid"] {
            for cell in cells_for(&buffer, needle) {
                assert_eq!(cell.fg, Color::Rgb(17, 17, 17));
                assert_eq!(cell.bg, Color::Rgb(238, 238, 238));
                assert!(!cell.modifier.intersects(Modifier::DIM | Modifier::REVERSED));
            }
        }
        assert_eq!(buffer[(0, 0)].fg, Color::Rgb(85, 255, 85));
    }

    #[test]
    fn reversed_selection_uses_base_colors_and_remains_visible_over_dim_content() {
        let mut app = app();
        app.theme = theme(
            r##"
version = 1
[styles.text]
fg = "#dddddd"
bg = "#111111"
modifiers = ["dim"]
[styles.text_muted]
fg = "#aaaaaa"
"##,
        );
        let buffer = render(&mut app, 80, 24);
        for needle in ["Synthetic profile", "example.invalid"] {
            for cell in cells_for(&buffer, needle) {
                assert_eq!(cell.fg, Color::Rgb(221, 221, 221));
                assert_eq!(cell.bg, Color::Rgb(17, 17, 17));
                assert!(cell.modifier.contains(Modifier::REVERSED));
                assert!(!cell.modifier.contains(Modifier::DIM));
            }
        }
    }

    #[test]
    fn selected_row_inverts_an_already_reversed_base() {
        let mut app = app();
        app.theme = theme(
            r##"
version = 1
[styles.text]
fg = "#dddddd"
bg = "#111111"
modifiers = ["reversed", "dim"]
"##,
        );
        let buffer = render(&mut app, 80, 24);
        for cell in cells_for(&buffer, "Synthetic profile") {
            assert!(!cell.modifier.intersects(Modifier::REVERSED | Modifier::DIM));
        }
    }

    #[test]
    fn edited_fields_and_certificate_warnings_use_distinct_roles() {
        let mut app = app();
        app.theme = theme(TEST_THEME);
        let mut form = super::super::EditForm::blank();
        form.editing_text = Some("Edited value".into());
        app.mode = Mode::Editing(Box::new(form));
        let buffer = render(&mut app, 80, 24);
        let cell = &cells_for(&buffer, "Edited value")[0];
        assert_eq!(cell.fg, Color::Rgb(17, 17, 17));
        assert_eq!(cell.bg, Color::Rgb(170, 255, 255));
        assert!(cell.modifier.contains(Modifier::BOLD));
        assert!(!cell.modifier.intersects(Modifier::DIM | Modifier::REVERSED));

        app.mode = Mode::CertificateMismatch(crate::freerdp::certificate::CertificateMismatch {
            profile_id: app.profiles[0].id,
            endpoint: app.profiles[0].endpoint.clone(),
            pinned_sha256: Some("SYNTHETIC_OLD".into()),
            presented_sha256: "SYNTHETIC_NEW".into(),
        });
        let buffer = render(&mut app, 80, 24);
        assert_eq!(
            cells_for(&buffer, "certificate warning")[0].fg,
            Color::Rgb(255, 255, 85)
        );
        assert_eq!(
            cells_for(&buffer, "SYNTHETIC_NEW")[0].fg,
            Color::Rgb(221, 221, 221)
        );
    }

    #[test]
    fn status_styles_follow_explicit_severity_rather_than_message_words() {
        for (severity, expected) in [
            (Severity::Neutral, Color::Rgb(221, 221, 221)),
            (Severity::Info, Color::Rgb(85, 170, 255)),
            (Severity::Success, Color::Rgb(85, 255, 170)),
            (Severity::Warning, Color::Rgb(255, 255, 85)),
            (Severity::Error, Color::Rgb(255, 85, 85)),
        ] {
            let mut app = app();
            app.theme = theme(TEST_THEME);
            app.status = "Status sample with error and success words".into();
            app.status_severity = severity;
            let buffer = render(&mut app, 100, 25);
            assert!(
                cells_for(&buffer, "Status sample")
                    .iter()
                    .all(|cell| cell.fg == expected)
            );
        }
    }

    #[test]
    fn modal_body_is_neutral_while_warning_and_error_headings_use_severity() {
        let mut app = app();
        app.theme = theme(TEST_THEME);
        app.theme_warning = Some(TextView::new(
            "Theme configuration warning",
            "Synthetic diagnostic".into(),
        ));
        let buffer = render(&mut app, 80, 24);
        assert_eq!(
            cells_for(&buffer, "Theme configuration warning")[0].fg,
            Color::Rgb(255, 255, 85)
        );
        assert_eq!(
            cells_for(&buffer, "Synthetic diagnostic")[0].fg,
            Color::Rgb(221, 221, 221)
        );
        app.error_dialog = Some(TextView::new("Error", "Action diagnostic".into()));
        let buffer = render(&mut app, 80, 24);
        assert_eq!(cells_for(&buffer, " Error ")[1].fg, Color::Rgb(255, 85, 85));
        assert_eq!(
            cells_for(&buffer, "Action diagnostic")[0].fg,
            Color::Rgb(221, 221, 221)
        );
    }

    #[test]
    fn themes_preserve_masking_and_render_at_small_sizes_and_after_resize() {
        for source in [
            include_str!("../../../themes/monochrome.toml"),
            include_str!("../../../themes/terminal.toml"),
            include_str!("../../../themes/brogrammer.toml"),
        ] {
            let mut app = app();
            app.theme = theme(source);
            for (width, height) in [(80, 24), (100, 25), (40, 12), (20, 6)] {
                render(&mut app, width, height);
                app.mode = Mode::Password {
                    slot: CredentialSlot::Main,
                    input: "synthetic-secret".to_string().into(),
                };
                let buffer = render(&mut app, width, height);
                let text: String = buffer
                    .content
                    .iter()
                    .map(ratatui::buffer::Cell::symbol)
                    .collect();
                assert!(!text.contains("synthetic-secret"));
                app.mode = Mode::Browsing;
            }
            app.error_dialog = Some(TextView::new(
                "Error",
                "Synthetic diagnostic line\n".repeat(40),
            ));
            render(&mut app, 40, 12);
            app.error_dialog.as_mut().unwrap().scroll = u16::MAX;
            render(&mut app, 100, 25);
            let dialog = app.error_dialog.as_ref().unwrap();
            assert!(dialog.scroll <= dialog.max_scroll);
        }
    }
}
