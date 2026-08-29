//! Ratatui frontend: a profile list that connects, tests, and stores passwords
//! through the same `session`/`credentials` functions the CLI uses (INV-8).

use super::terminal::TerminalGuard;
use crate::config::ConfigStore;
use crate::credentials::{forget_encrypted, store_encrypted_password};
use crate::model::Profile;
use crate::profile_store::ProfileStore;
use crate::session::{connect_profile, test_profile};
use ratatui::Terminal;
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Style, Stylize as _};
use ratatui::text::Line;
use ratatui::widgets::{Block, List, ListItem, ListState, Paragraph};
use secrecy::SecretString;
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
    let mut app = App::new(profiles, executable, config_root.to_path_buf());

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

enum Mode {
    Browsing,
    Password(String),
}

struct App {
    profiles: Vec<Profile>,
    selected: ListState,
    status: String,
    executable: PathBuf,
    config_root: PathBuf,
    mode: Mode,
}

impl App {
    fn new(profiles: Vec<Profile>, executable: PathBuf, config_root: PathBuf) -> Self {
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
            config_root,
            mode: Mode::Browsing,
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
            match self.mode {
                Mode::Browsing => {
                    if self.handle_browsing(key) {
                        return Ok(());
                    }
                }
                Mode::Password(_) => self.handle_password(key),
            }
        }
    }

    /// Returns true when the app should exit.
    fn handle_browsing(&mut self, key: KeyEvent) -> bool {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return true;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => return true,
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(true),
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(false),
            KeyCode::Enter | KeyCode::Char('c') => self.connect(),
            KeyCode::Char('t') => self.test(),
            KeyCode::Char('p') => self.begin_password(),
            _ => {}
        }
        false
    }

    fn handle_password(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Browsing;
                self.status = "Password entry cancelled.".into();
            }
            KeyCode::Enter => self.commit_password(),
            KeyCode::Backspace => {
                if let Mode::Password(input) = &mut self.mode {
                    input.pop();
                }
            }
            KeyCode::Char(character) => {
                if let Mode::Password(input) = &mut self.mode {
                    input.push(character);
                }
            }
            _ => {}
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

    fn begin_password(&mut self) {
        let Some(name) = self.current().map(|profile| profile.name.clone()) else {
            return;
        };
        self.mode = Mode::Password(String::new());
        self.status = format!("Enter password for {name} — Enter to save, Esc to cancel");
    }

    fn commit_password(&mut self) {
        let password = match &self.mode {
            Mode::Password(input) => input.clone(),
            Mode::Browsing => return,
        };
        self.mode = Mode::Browsing;
        if password.is_empty() {
            self.status = "Password entry cancelled (empty).".into();
            return;
        }
        let Some(index) = self.selected.selected() else {
            return;
        };
        let name = self.profiles[index].name.clone();
        self.status = match self.save_password(index, &password) {
            Ok(()) => format!("Saved a password for {name}"),
            Err(error) => format!("Could not save password: {error}"),
        };
    }

    fn save_password(&mut self, index: usize, password: &str) -> Result<(), String> {
        let reference = store_encrypted_password(
            self.config_root.as_path(),
            &SecretString::from(password.to_owned()),
        )
        .map_err(|error| error.to_string())?;
        let mut profile = self.profiles[index].clone();
        let previous = profile.credential.replace(reference);
        ProfileStore::new(ConfigStore::new(self.config_root.as_path()))
            .upsert(profile.clone())
            .map_err(|error| error.to_string())?;
        // Keep the in-memory profile consistent so the next connect uses it.
        self.profiles[index] = profile;
        if let Some(previous) = previous {
            forget_encrypted(self.config_root.as_path(), previous);
        }
        Ok(())
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

        let (title, body) = match &self.mode {
            Mode::Browsing => (
                " ↑/↓ move · Enter/c connect · t test · p password · q quit ",
                self.status.clone(),
            ),
            Mode::Password(input) => (
                " typing password · Enter save · Esc cancel ",
                format!("Password: {}", "*".repeat(input.chars().count())),
            ),
        };
        let footer = Paragraph::new(body).block(Block::bordered().title(title));
        frame.render_widget(footer, areas[1]);
    }
}

#[cfg(test)]
mod tests {
    use super::{App, Mode};
    use crate::config::ConfigStore;
    use crate::model::{
        DeviceConfig, DisplayConfig, Endpoint, IdentityConfig, Profile, ProfileId, Route,
        SecurityConfig,
    };
    use crate::profile_store::ProfileStore;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn app_with(names: &[&str]) -> App {
        let profiles = names
            .iter()
            .map(|name| {
                let mut profile = sample_profile();
                profile.name = (*name).to_string();
                profile
            })
            .collect();
        App::new(
            profiles,
            PathBuf::from("/usr/bin/rdp-tui"),
            PathBuf::from("/tmp/rdp-tui-config"),
        )
    }

    fn rendered(app: &mut App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect()
    }

    #[test]
    fn renders_profiles_and_help_at_80x24() {
        let mut app = app_with(&["Anima", "Compono"]);
        let screen = rendered(&mut app, 80, 24);
        assert!(screen.contains("Anima"));
        assert!(screen.contains("Compono"));
        assert!(screen.contains("connect"));
    }

    #[test]
    fn renders_import_hint_with_no_profiles() {
        let mut app = app_with(&[]);
        assert!(rendered(&mut app, 80, 24).contains("migrate python"));
    }

    #[test]
    fn renders_many_and_unicode_names_without_panicking() {
        let mut app = app_with(&["Straße-Über-Café", "b", "c", "d", "e", "f", "g", "h"]);
        assert!(rendered(&mut app, 40, 10).contains("Stra"));
    }

    #[test]
    fn password_mode_masks_the_typed_input() {
        let mut app = app_with(&["Anima"]);
        app.begin_password();
        for character in "secret".chars() {
            app.handle_password(press(KeyCode::Char(character)));
        }
        let screen = rendered(&mut app, 80, 24);
        assert!(screen.contains("******"));
        assert!(!screen.contains("secret"));
    }

    fn sample_profile() -> Profile {
        Profile {
            id: ProfileId::generate(),
            name: "Sample".into(),
            endpoint: "10.0.0.5:3389".parse::<Endpoint>().unwrap(),
            identity: IdentityConfig::default(),
            route: Route::Direct,
            display: DisplayConfig::default(),
            devices: DeviceConfig::default(),
            security: SecurityConfig::default(),
            credential: None,
        }
    }

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn typing_and_saving_a_password_pins_a_credential_on_disk_and_in_memory() {
        let dir = TempDir::new().unwrap();
        let config_root = dir.path().to_path_buf();
        let store = ProfileStore::new(ConfigStore::new(config_root.as_path()));
        let profile = sample_profile();
        let id = profile.id;
        store.upsert(profile.clone()).unwrap();

        let mut app = App::new(
            vec![profile],
            PathBuf::from("/usr/bin/rdp-tui"),
            config_root.clone(),
        );
        app.begin_password();
        assert!(matches!(app.mode, Mode::Password(_)));
        for character in "hunter2".chars() {
            app.handle_password(press(KeyCode::Char(character)));
        }
        app.handle_password(press(KeyCode::Enter));

        assert!(matches!(app.mode, Mode::Browsing));
        assert!(app.profiles[0].credential.is_some());
        assert!(store.get(id).unwrap().unwrap().credential.is_some());
    }
}
