//! Pure rendering for the TUI, separated from controller and state transitions.

use super::{App, FIELDS, Mode};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, List, ListItem, ListState, Paragraph};

pub(super) fn draw(app: &mut App, frame: &mut ratatui::Frame) {
    let areas = Layout::vertical([Constraint::Min(3), Constraint::Length(3)]).split(frame.area());

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
    } else if let Mode::Status(text) = &app.mode {
        frame.render_widget(
            Paragraph::new(text.clone()).block(Block::bordered().title(" status ")),
            areas[0],
        );
    } else if let Mode::CertificateMismatch(mismatch) = &app.mode {
        let text = format!(
            "Changed certificate for {}\n\nPinned SHA-256:\n{}\n\nPresented SHA-256:\n{}\n\nVerify this fingerprint through a trusted channel. Press T only if it matches.",
            mismatch.endpoint,
            mismatch.pinned_sha256.as_deref().unwrap_or("unavailable"),
            mismatch.presented_sha256,
        );
        frame.render_widget(
            Paragraph::new(text).block(Block::bordered().title(" certificate warning ")),
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

    let (title, body) = match &app.mode {
        Mode::Browsing => (
            " Enter connect · a/e add/edit · c clone · d delete · f find · i/x import/export · s status · h history · t test · D deep-test · p/g pass · T cert · ? help · q quit ".to_string(),
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
        Mode::Status(_) => (" any key to return ".to_string(), String::new()),
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
    frame.render_widget(
        Paragraph::new(body).block(Block::bordered().title(title)),
        areas[1],
    );
}
