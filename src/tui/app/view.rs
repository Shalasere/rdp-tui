//! Pure rendering for the TUI, separated from controller and state transitions.

use super::{App, FIELDS, Mode, TextView};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap};

pub(super) fn draw(app: &mut App, frame: &mut ratatui::Frame) {
    let footer = footer(app, frame.area().width);
    let footer_height = u16::try_from(footer.line_count(frame.area().width.saturating_sub(2)))
        .unwrap_or(u16::MAX)
        .saturating_add(2)
        .min(frame.area().height.saturating_sub(3));
    let footer = footer.block(Block::bordered());
    let areas = Layout::vertical([Constraint::Min(3), Constraint::Length(footer_height)])
        .split(frame.area());

    if let Mode::Editing(form) = &app.mode {
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
        let list = List::new(items)
            .block(Block::bordered().title(title))
            .highlight_symbol("> ")
            .highlight_style(Style::new().reversed());
        let mut state = ListState::default();
        state.select(Some(form.field));
        frame.render_stateful_widget(list, areas[0], &mut state);
    } else if let Mode::Status(details) = &mut app.mode {
        let block = Block::bordered().title(format!(" {} ", details.title));
        let inner = block.inner(areas[0]);
        update_scroll(details, inner);
        frame.render_widget(block, areas[0]);
        frame.render_widget(
            Paragraph::new(details.message.as_str())
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
                .wrap(Wrap { trim: false })
                .block(Block::bordered().title(" certificate warning ")),
            areas[0],
        );
    } else {
        let items: Vec<ListItem> = app
            .visible
            .iter()
            .map(|&index| {
                let profile = &app.profiles[index];
                ListItem::new(Line::from(format!(
                    "{}    {}",
                    profile.name, profile.endpoint
                )))
            })
            .collect();
        let list = List::new(items)
            .block(Block::bordered().title(" rdp-tui — profiles "))
            .highlight_symbol("> ")
            .highlight_style(Style::new().reversed());
        frame.render_stateful_widget(list, areas[0], &mut app.selected);
    }

    frame.render_widget(footer, areas[1]);
    if let Some(dialog) = app.error_dialog.as_mut() {
        draw_error_dialog(dialog, frame);
    }
}

fn footer(app: &App, width: u16) -> Paragraph<'static> {
    let (title, body) = match &app.mode {
        Mode::Browsing => (
            " Enter connect · a/e add/edit · c clone · d delete · f find · i/x import/export · s status · h history · m messages · t test · D deep-test · p/g pass · T cert · ? help · q quit ".to_string(),
            app.status.clone(),
        ),
        Mode::Password { input, .. } => (
            " typing password · Enter save · Esc cancel ".to_string(),
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
    let footer_text = if body.is_empty() {
        title.trim().to_string()
    } else {
        format!("{}\n{body}", title.trim())
    };
    let footer_text = if app.notification_pending {
        let hint = if matches!(app.mode, Mode::Browsing) {
            "Error · m details"
        } else {
            "Error details pending"
        };
        format!("{hint}\n{footer_text}")
    } else {
        footer_text
    };
    Paragraph::new(footer_text).wrap(Wrap { trim: true })
}

fn draw_error_dialog(dialog: &mut TextView, frame: &mut ratatui::Frame) {
    let area = frame.area();
    let width = area.width.saturating_sub(4).max(20).min(area.width);
    let inner_width = width.saturating_sub(2);
    let content_height = u16::try_from(
        Paragraph::new(dialog.message.as_str())
            .wrap(Wrap { trim: false })
            .line_count(inner_width),
    )
    .unwrap_or(u16::MAX);
    let instructions = if inner_width < 60 {
        "↑↓ scroll · Enter/Esc close"
    } else {
        "↑↓ scroll · PgUp/PgDn page · Home/End · Enter/Esc close"
    };
    let hints = Paragraph::new(instructions).wrap(Wrap { trim: true });
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
    let block = Block::bordered().title(format!(" {} ", dialog.title));
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
