//! Ratatui frontend: a profile list that connects and tests through the same
//! `session` functions the CLI uses (INV-8).

use super::terminal::TerminalGuard;
use crate::config::ConfigStore;
use crate::model::Profile;
use crate::profile_store::ProfileStore;
use crate::session::{connect_profile, test_profile};
use ratatui::Terminal;
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Style, Stylize as _};
use ratatui::text::Line;
use ratatui::widgets::{Block, List, ListItem, ListState, Paragraph};
use std::io;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::time::Duration;

const TEST_TIMEOUT: Duration = Duration::from_secs(5);

/// Run the TUI against the profiles stored under `config_root`.
///
/// # Errors
///
/// Returns an I/O error when profiles cannot be loaded or the terminal fails.
pub fn run(config_root: &Path) -> io::Result<()> {
    let store = ProfileStore::new(ConfigStore::new(config_root));
    let profiles = store.list().map_err(io::Error::other)?;
    let executable = std::env::current_exe()?;
    let mut app = App::new(profiles, executable);

    let mut guard = TerminalGuard::enter()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    // The catch_unwind boundary restores the terminal exactly once after a panic
    // in the render/event path, then re-raises the panic (INV-12).
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| app.event_loop(&mut terminal)));
    guard.restore();
    match outcome {
        Ok(result) => result,
        Err(payload) => panic::resume_unwind(payload),
    }
}

struct App {
    profiles: Vec<Profile>,
    selected: ListState,
    status: String,
    executable: PathBuf,
}

impl App {
    fn new(profiles: Vec<Profile>, executable: PathBuf) -> Self {
        let mut selected = ListState::default();
        let status = if profiles.is_empty() {
            "No profiles yet. Import with: rdp-tui migrate python".to_string()
        } else {
            selected.select(Some(0));
            "Ready.".to_string()
        };
        Self {
            profiles,
            selected,
            status,
            executable,
        }
    }

    fn event_loop<B: Backend>(&mut self, terminal: &mut Terminal<B>) -> io::Result<()> {
        loop {
            terminal.draw(|frame| self.draw(frame))?;
            let Event::Key(key) = event::read()? else {
                continue;
            };
            if key.kind != KeyEventKind::Press {
                continue;
            }
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                return Ok(());
            }
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                KeyCode::Down | KeyCode::Char('j') => self.move_selection(true),
                KeyCode::Up | KeyCode::Char('k') => self.move_selection(false),
                KeyCode::Enter | KeyCode::Char('c') => self.connect(),
                KeyCode::Char('t') => self.test(),
                _ => {}
            }
        }
    }

    fn move_selection(&mut self, forward: bool) {
        let count = self.profiles.len();
        if count == 0 {
            return;
        }
        let current = self.selected.selected().unwrap_or(0);
        let next = if forward {
            (current + 1) % count
        } else {
            (current + count - 1) % count
        };
        self.selected.select(Some(next));
    }

    fn current(&self) -> Option<&Profile> {
        self.selected
            .selected()
            .and_then(|index| self.profiles.get(index))
    }

    fn connect(&mut self) {
        let Some(profile) = self.current().cloned() else {
            return;
        };
        self.status = match connect_profile(&profile, &self.executable) {
            Ok(session) => format!("Launched session {session} for {}", profile.name),
            Err(error) => format!("Connect failed: {error}"),
        };
    }

    fn test(&mut self) {
        let Some(profile) = self.current().cloned() else {
            return;
        };
        self.status = match test_profile(&profile, TEST_TIMEOUT) {
            Ok(()) => format!("{} is reachable", profile.name),
            Err(error) => format!("{}: {error}", profile.name),
        };
    }

    fn draw(&mut self, frame: &mut ratatui::Frame) {
        let areas =
            Layout::vertical([Constraint::Min(3), Constraint::Length(3)]).split(frame.area());
        let items: Vec<ListItem> = self
            .profiles
            .iter()
            .map(|profile| {
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
        frame.render_stateful_widget(list, areas[0], &mut self.selected);

        let footer = Paragraph::new(self.status.as_str())
            .block(Block::bordered().title(" ↑/↓ move · Enter/c connect · t test · q quit "));
        frame.render_widget(footer, areas[1]);
    }
}
