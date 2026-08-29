//! Ratatui frontend: a profile list that connects, tests, stores passwords, and
//! manages profiles (add/edit/clone/delete/find) through the same
//! `session`/`credentials`/`profile_store` functions the CLI uses (INV-8).

use super::terminal::TerminalGuard;
use crate::config::ConfigStore;
use crate::credentials::{forget_encrypted, store_encrypted_password};
use crate::model::{
    CertificatePolicy, DeviceConfig, DisplayConfig, Endpoint, IdentityConfig, Profile, ProfileId,
    Renderer, Route, SecurityConfig,
};
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
use std::fmt::Write as _;
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

/// A single-line text prompt (visible input) with the action to run on Enter.
#[derive(Clone)]
enum PromptAction {
    Find,
    ConfirmDelete(ProfileId),
    Import,
    Export(ProfileId),
}

enum Mode {
    Browsing,
    Password(String),
    Prompt {
        label: String,
        input: String,
        action: PromptAction,
    },
    Editing(EditForm),
    /// A read-only details/session overlay; any key returns to browsing.
    Status(String),
}

/// The seven directly-editable fields of a profile, in display order.
const FIELD_LABELS: [&str; 7] = [
    "Name",
    "Host",
    "Username",
    "Domain",
    "Renderer",
    "Fullscreen",
    "Certificate",
];

/// A working copy of a profile being added (`target` is `None`) or edited
/// (`target` is the existing id). Only the common fields are exposed here;
/// route, device, and advanced display settings stay in the CLI `set` command.
#[derive(Clone)]
struct EditForm {
    target: Option<ProfileId>,
    name: String,
    host: String,
    username: String,
    domain: String,
    renderer: Renderer,
    fullscreen: bool,
    certificate_policy: CertificatePolicy,
    field: usize,
    /// `Some` while the highlighted text field is being typed into.
    editing_text: Option<String>,
}

impl EditForm {
    fn blank() -> Self {
        let display = DisplayConfig::default();
        let security = SecurityConfig::default();
        Self {
            target: None,
            name: String::new(),
            host: String::new(),
            username: String::new(),
            domain: String::new(),
            renderer: display.renderer,
            fullscreen: display.fullscreen,
            certificate_policy: security.certificate_policy,
            field: 0,
            editing_text: None,
        }
    }

    fn from_profile(profile: &Profile) -> Self {
        Self {
            target: Some(profile.id),
            name: profile.name.clone(),
            host: profile.endpoint.to_string(),
            username: profile.identity.username.clone(),
            domain: profile.identity.domain.clone(),
            renderer: profile.display.renderer,
            fullscreen: profile.display.fullscreen,
            certificate_policy: profile.security.certificate_policy,
            field: 0,
            editing_text: None,
        }
    }

    const fn is_text_field(field: usize) -> bool {
        field <= 3
    }

    fn move_field(&mut self, forward: bool) {
        let count = FIELD_LABELS.len();
        self.field = if forward {
            (self.field + 1) % count
        } else {
            (self.field + count - 1) % count
        };
    }

    fn text_field(&self, field: usize) -> String {
        match field {
            0 => self.name.clone(),
            1 => self.host.clone(),
            2 => self.username.clone(),
            3 => self.domain.clone(),
            _ => String::new(),
        }
    }

    fn set_text_field(&mut self, text: String) {
        match self.field {
            0 => self.name = text,
            1 => self.host = text,
            2 => self.username = text,
            3 => self.domain = text,
            _ => {}
        }
    }

    /// Toggle or advance the highlighted non-text field. A no-op on text fields.
    fn cycle(&mut self) {
        match self.field {
            4 => self.renderer = next_renderer(self.renderer),
            5 => self.fullscreen = !self.fullscreen,
            6 => self.certificate_policy = next_policy(self.certificate_policy),
            _ => {}
        }
    }

    fn display_value(&self, field: usize) -> String {
        match field {
            0 => self.name.clone(),
            1 => self.host.clone(),
            2 => self.username.clone(),
            3 => self.domain.clone(),
            4 => renderer_label(self.renderer).to_string(),
            5 => if self.fullscreen { "yes" } else { "no" }.to_string(),
            6 => policy_label(self.certificate_policy).to_string(),
            _ => String::new(),
        }
    }
}

struct App {
    profiles: Vec<Profile>,
    /// Indices into `profiles` that pass the current filter, in display order.
    visible: Vec<usize>,
    filter: String,
    selected: ListState,
    status: String,
    executable: PathBuf,
    config_root: PathBuf,
    mode: Mode,
}

impl App {
    fn new(profiles: Vec<Profile>, executable: PathBuf, config_root: PathBuf) -> Self {
        let mut app = Self {
            profiles,
            visible: Vec::new(),
            filter: String::new(),
            selected: ListState::default(),
            status: String::new(),
            executable,
            config_root,
            mode: Mode::Browsing,
        };
        app.recompute_visible();
        app.status = app.describe_current();
        app
    }

    fn store(&self) -> ProfileStore {
        ProfileStore::new(ConfigStore::new(self.config_root.as_path()))
    }

    /// Rebuild the visible index list from the current filter and keep the
    /// selection in range.
    fn recompute_visible(&mut self) {
        let query = self.filter.to_lowercase();
        self.visible = self
            .profiles
            .iter()
            .enumerate()
            .filter(|(_, profile)| query.is_empty() || profile_matches(profile, &query))
            .map(|(index, _)| index)
            .collect();
        if self.visible.is_empty() {
            self.selected.select(None);
        } else {
            let position = self
                .selected
                .selected()
                .unwrap_or(0)
                .min(self.visible.len() - 1);
            self.selected.select(Some(position));
        }
    }

    /// Reload profiles from disk (after a mutation) and reapply the filter.
    fn reload(&mut self) {
        match self.store().list() {
            Ok(profiles) => self.profiles = profiles,
            Err(error) => self.status = format!("Could not reload profiles: {error}"),
        }
        self.recompute_visible();
    }

    fn select_id(&mut self, id: ProfileId) {
        if let Some(position) = self
            .visible
            .iter()
            .position(|&index| self.profiles[index].id == id)
        {
            self.selected.select(Some(position));
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
            match &self.mode {
                Mode::Browsing => {
                    if self.handle_browsing(key) {
                        return Ok(());
                    }
                }
                Mode::Password(_) => self.handle_password(key),
                Mode::Prompt { .. } => self.handle_prompt(key),
                Mode::Editing(_) => self.handle_editing(key),
                Mode::Status(_) => {
                    self.mode = Mode::Browsing;
                    self.status = self.describe_current();
                }
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
            KeyCode::Enter => self.connect(),
            KeyCode::Char('t') => self.test(),
            KeyCode::Char('p') => self.begin_password(),
            KeyCode::Char('a') => self.begin_add(),
            KeyCode::Char('e') => self.begin_edit(),
            KeyCode::Char('c') => self.clone_current(),
            KeyCode::Char('d') => self.begin_delete(),
            KeyCode::Char('f' | '/') => self.begin_find(),
            KeyCode::Char('i') => self.begin_import(),
            KeyCode::Char('x') => self.begin_export(),
            KeyCode::Char('s') => self.begin_status(),
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

    fn handle_prompt(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.mode = Mode::Browsing;
                self.status = "Cancelled.".into();
            }
            KeyCode::Enter => self.commit_prompt(),
            KeyCode::Backspace => {
                if let Mode::Prompt { input, .. } = &mut self.mode {
                    input.pop();
                }
            }
            KeyCode::Char(character) => {
                if let Mode::Prompt { input, .. } = &mut self.mode {
                    input.push(character);
                }
            }
            _ => {}
        }
    }

    fn commit_prompt(&mut self) {
        let (action, input) = match &self.mode {
            Mode::Prompt { action, input, .. } => (action.clone(), input.clone()),
            _ => return,
        };
        self.mode = Mode::Browsing;
        match action {
            PromptAction::Find => {
                self.filter = input.trim().to_string();
                self.recompute_visible();
                self.status = if self.filter.is_empty() {
                    self.describe_current()
                } else {
                    format!("Filter: {} ({} shown)", self.filter, self.visible.len())
                };
            }
            PromptAction::ConfirmDelete(id) => {
                self.perform_delete(id, input.trim().eq_ignore_ascii_case("yes"));
            }
            PromptAction::Import => self.perform_import(input.trim()),
            PromptAction::Export(id) => self.perform_export(id, input.trim()),
        }
    }

    fn begin_import(&mut self) {
        self.mode = Mode::Prompt {
            label: "Import a Remmina/.rdp file or a directory".into(),
            input: String::new(),
            action: PromptAction::Import,
        };
    }

    fn perform_import(&mut self, path: &str) {
        if path.is_empty() {
            self.status = "Import cancelled.".into();
            return;
        }
        match crate::config::import::import_path(Path::new(path)) {
            Ok(profiles) => {
                let store = self.store();
                let existing = self.profiles.clone();
                let (mut added, mut skipped) = (0_usize, 0_usize);
                for profile in profiles {
                    if existing
                        .iter()
                        .any(|current| same_except_id(current, &profile))
                    {
                        skipped += 1;
                    } else if store.upsert(profile).is_ok() {
                        added += 1;
                    }
                }
                self.reload();
                self.status = format!("Imported {added}, skipped {skipped} unchanged");
            }
            Err(error) => self.status = format!("Import failed: {error}"),
        }
    }

    fn begin_export(&mut self) {
        let Some(profile) = self.current() else {
            self.status = "No profile to export.".into();
            return;
        };
        self.mode = Mode::Prompt {
            label: format!("Export {} to .rdp path (password excluded)", profile.name),
            input: format!("{}.rdp", profile.name.replace(' ', "_")),
            action: PromptAction::Export(profile.id),
        };
    }

    fn perform_export(&mut self, id: ProfileId, path: &str) {
        if path.is_empty() {
            self.status = "Export cancelled.".into();
            return;
        }
        let Some(profile) = self.profiles.iter().find(|profile| profile.id == id) else {
            self.status = "Profile is no longer available.".into();
            return;
        };
        let name = profile.name.clone();
        let contents = crate::config::import::export_rdp(profile);
        let mut destination = PathBuf::from(path);
        if destination.extension().and_then(std::ffi::OsStr::to_str) != Some("rdp") {
            destination.set_extension("rdp");
        }
        self.status = match std::fs::write(&destination, contents) {
            Ok(()) => format!("Exported {name} to {} (no password)", destination.display()),
            Err(error) => format!("Export failed: {error}"),
        };
    }

    fn begin_status(&mut self) {
        let mut text = String::new();
        if let Some(profile) = self.current() {
            let _ = writeln!(text, "Profile: {}", profile.name);
            let _ = writeln!(text, "Endpoint: {}", profile.endpoint);
            let _ = writeln!(text, "Route: {:?}", profile.route);
            let _ = writeln!(
                text,
                "Renderer: {}   Fullscreen: {}",
                renderer_label(profile.display.renderer),
                if profile.display.fullscreen {
                    "yes"
                } else {
                    "no"
                }
            );
            let _ = writeln!(
                text,
                "Certificate: {}   Password: {}",
                policy_label(profile.security.certificate_policy),
                if profile.credential.is_some() {
                    "saved"
                } else {
                    "none"
                }
            );
        } else {
            let _ = writeln!(text, "No profile selected.");
        }
        let _ = writeln!(text, "\nActive sessions:");
        let mut any = false;
        if let Some(dir) = crate::session::record::sessions_dir() {
            for session in crate::session::scan_sessions(&dir) {
                any = true;
                let _ = writeln!(
                    text,
                    "  {} — {:?} (profile {})",
                    session.record.session_id, session.health, session.record.profile_id
                );
            }
        }
        if !any {
            let _ = writeln!(text, "  none active");
        }
        self.mode = Mode::Status(text);
    }

    fn handle_editing(&mut self, key: KeyEvent) {
        // Typing into a text field: characters, Backspace, Enter (commit), Esc (abort).
        if let Mode::Editing(form) = &mut self.mode
            && form.editing_text.is_some()
        {
            match key.code {
                KeyCode::Esc => form.editing_text = None,
                KeyCode::Enter => {
                    if let Some(text) = form.editing_text.take() {
                        form.set_text_field(text);
                    }
                }
                KeyCode::Backspace => {
                    if let Some(text) = &mut form.editing_text {
                        text.pop();
                    }
                }
                KeyCode::Char(character) => {
                    if let Some(text) = &mut form.editing_text {
                        text.push(character);
                    }
                }
                _ => {}
            }
            return;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.mode = Mode::Browsing;
                self.status = "Edit cancelled.".into();
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Mode::Editing(form) = &mut self.mode {
                    form.move_field(true);
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if let Mode::Editing(form) = &mut self.mode {
                    form.move_field(false);
                }
            }
            KeyCode::Enter => self.activate_field(),
            KeyCode::Char(' ') => {
                if let Mode::Editing(form) = &mut self.mode {
                    form.cycle();
                }
            }
            KeyCode::Char('A') => self.save_form(),
            _ => {}
        }
    }

    /// Enter on a field: begin typing a text field, or cycle a toggle/enum.
    fn activate_field(&mut self) {
        if let Mode::Editing(form) = &mut self.mode {
            if EditForm::is_text_field(form.field) {
                form.editing_text = Some(form.text_field(form.field));
            } else {
                form.cycle();
            }
        }
    }

    fn save_form(&mut self) {
        let form = match &self.mode {
            Mode::Editing(form) => form.clone(),
            _ => return,
        };
        if form.name.trim().is_empty() {
            self.status = "A name is required.".into();
            return;
        }
        let endpoint = match form.host.trim().parse::<Endpoint>() {
            Ok(endpoint) => endpoint,
            Err(error) => {
                self.status = format!("Invalid host: {error}");
                return;
            }
        };
        let store = self.store();
        // Editing preserves the profile's non-form fields (route, devices,
        // resolution, pinned credential); adding starts from defaults.
        let mut profile = match form.target {
            Some(id) => match store.get(id) {
                Ok(Some(profile)) => profile,
                _ => blank_profile(),
            },
            None => blank_profile(),
        };
        profile.name = form.name.trim().to_string();
        profile.endpoint = endpoint;
        form.username.clone_into(&mut profile.identity.username);
        form.domain.clone_into(&mut profile.identity.domain);
        profile.display.renderer = form.renderer;
        profile.display.fullscreen = form.fullscreen;
        profile.security.certificate_policy = form.certificate_policy;
        let id = profile.id;
        let name = profile.name.clone();
        let adding = form.target.is_none();
        match store.upsert(profile) {
            Ok(()) => {
                self.mode = Mode::Browsing;
                self.reload();
                self.select_id(id);
                self.status = if adding {
                    format!("Added {name}")
                } else {
                    format!("Saved {name}")
                };
            }
            Err(error) => self.status = format!("Could not save: {error}"),
        }
    }

    fn move_selection(&mut self, forward: bool) {
        let count = self.visible.len();
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
        self.status = self.describe_current();
    }

    fn current_index(&self) -> Option<usize> {
        self.selected
            .selected()
            .and_then(|position| self.visible.get(position).copied())
    }

    fn current(&self) -> Option<&Profile> {
        self.current_index().map(|index| &self.profiles[index])
    }

    fn describe_current(&self) -> String {
        match self.current() {
            Some(profile) => {
                let password = if profile.credential.is_some() {
                    "saved"
                } else {
                    "none"
                };
                format!(
                    "{} · {} · cert:{} · password:{password}",
                    profile.name,
                    profile.endpoint,
                    policy_label(profile.security.certificate_policy),
                )
            }
            None if self.profiles.is_empty() => "No profiles yet. Press a to add one.".to_string(),
            None => format!("No matches for filter: {}", self.filter),
        }
    }

    fn begin_add(&mut self) {
        self.mode = Mode::Editing(EditForm::blank());
        self.status = "Adding a profile — A to accept, Esc to cancel.".into();
    }

    fn begin_edit(&mut self) {
        let Some(profile) = self.current() else {
            self.status = "No profile to edit.".into();
            return;
        };
        let name = profile.name.clone();
        self.mode = Mode::Editing(EditForm::from_profile(profile));
        self.status = format!("Editing {name} — A to accept, Esc to cancel.");
    }

    fn clone_current(&mut self) {
        let Some(source) = self.current() else {
            return;
        };
        let mut copy = source.clone();
        copy.id = ProfileId::generate();
        copy.name = format!("{} (copy)", copy.name);
        // A clone starts without the source's secret so deleting either profile
        // cannot forget a shared credential file.
        copy.credential = None;
        let (id, name) = (copy.id, copy.name.clone());
        match self.store().upsert(copy) {
            Ok(()) => {
                self.reload();
                self.select_id(id);
                self.status = format!("Cloned to {name}");
            }
            Err(error) => self.status = format!("Could not clone: {error}"),
        }
    }

    fn begin_delete(&mut self) {
        let Some(profile) = self.current() else {
            return;
        };
        self.mode = Mode::Prompt {
            label: format!("Delete {}? type yes to confirm", profile.name),
            input: String::new(),
            action: PromptAction::ConfirmDelete(profile.id),
        };
    }

    fn perform_delete(&mut self, id: ProfileId, confirmed: bool) {
        if !confirmed {
            self.status = "Delete cancelled.".into();
            return;
        }
        let removed = self.profiles.iter().find(|profile| profile.id == id);
        let (name, credential) = match removed {
            Some(profile) => (profile.name.clone(), profile.credential),
            None => (id.to_string(), None),
        };
        match self.store().remove(id) {
            Ok(_) => {
                if let Some(credential) = credential {
                    forget_encrypted(self.config_root.as_path(), credential);
                }
                self.reload();
                self.status = format!("Deleted {name}");
            }
            Err(error) => self.status = format!("Could not delete: {error}"),
        }
    }

    fn begin_find(&mut self) {
        self.mode = Mode::Prompt {
            label: "Filter by name, host, user, or domain (blank clears)".into(),
            input: self.filter.clone(),
            action: PromptAction::Find,
        };
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
            _ => return,
        };
        self.mode = Mode::Browsing;
        if password.is_empty() {
            self.status = "Password entry cancelled (empty).".into();
            return;
        }
        let Some(index) = self.current_index() else {
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
        self.store()
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

        if let Mode::Editing(form) = &self.mode {
            let items: Vec<ListItem> = FIELD_LABELS
                .iter()
                .enumerate()
                .map(|(index, label)| {
                    let shown = if form.field == index && form.editing_text.is_some() {
                        format!("{label}: {}_", form.editing_text.as_deref().unwrap_or(""))
                    } else {
                        format!("{label}: {}", form.display_value(index))
                    };
                    let item = ListItem::new(Line::from(shown));
                    if form.field == index {
                        item.style(Style::new().reversed())
                    } else {
                        item
                    }
                })
                .collect();
            let title = if form.target.is_some() {
                " edit profile "
            } else {
                " add profile "
            };
            frame.render_widget(
                List::new(items).block(Block::bordered().title(title)),
                areas[0],
            );
        } else if let Mode::Status(text) = &self.mode {
            let text = text.clone();
            frame.render_widget(
                Paragraph::new(text).block(Block::bordered().title(" status ")),
                areas[0],
            );
        } else {
            let items: Vec<ListItem> = self
                .visible
                .iter()
                .map(|&index| {
                    let profile = &self.profiles[index];
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
        }

        let (title, body) = match &self.mode {
            Mode::Browsing => (
                " Enter connect · a/e add/edit · c clone · d delete · f find · i/x import/export · s status · t test · p pass · q quit ",
                self.status.clone(),
            ),
            Mode::Status(_) => (" any key to return ", String::new()),
            Mode::Password(input) => (
                " typing password · Enter save · Esc cancel ",
                format!("Password: {}", "*".repeat(input.chars().count())),
            ),
            Mode::Prompt { label, input, .. } => {
                (" Enter confirm · Esc cancel ", format!("{label}: {input}"))
            }
            Mode::Editing(form) => (
                if form.editing_text.is_some() {
                    " typing · Enter set · Esc cancel field "
                } else {
                    " ↑/↓ field · Enter/Space edit · A accept · Esc cancel "
                },
                self.status.clone(),
            ),
        };
        let footer = Paragraph::new(body).block(Block::bordered().title(title));
        frame.render_widget(footer, areas[1]);
    }
}

fn blank_profile() -> Profile {
    Profile {
        id: ProfileId::generate(),
        name: String::new(),
        // Overwritten from the form's parsed host before the profile is saved.
        endpoint: "127.0.0.1:3389"
            .parse::<Endpoint>()
            .expect("literal endpoint parses"),
        identity: IdentityConfig::default(),
        route: Route::Direct,
        display: DisplayConfig::default(),
        devices: DeviceConfig::default(),
        security: SecurityConfig::default(),
        credential: None,
    }
}

/// Two profiles are duplicates if they match on everything but their id — the
/// same dedup rule the CLI importer uses, so re-importing is idempotent.
fn same_except_id(current: &Profile, incoming: &Profile) -> bool {
    let mut incoming = incoming.clone();
    incoming.id = current.id;
    current == &incoming
}

fn profile_matches(profile: &Profile, query: &str) -> bool {
    profile.name.to_lowercase().contains(query)
        || profile.endpoint.to_string().to_lowercase().contains(query)
        || profile.identity.username.to_lowercase().contains(query)
        || profile.identity.domain.to_lowercase().contains(query)
}

const fn renderer_label(renderer: Renderer) -> &'static str {
    match renderer {
        Renderer::WaylandSdl => "wayland_sdl",
        Renderer::X11 => "x11",
    }
}

const fn next_renderer(renderer: Renderer) -> Renderer {
    match renderer {
        Renderer::WaylandSdl => Renderer::X11,
        Renderer::X11 => Renderer::WaylandSdl,
    }
}

const fn policy_label(policy: CertificatePolicy) -> &'static str {
    match policy {
        CertificatePolicy::Tofu => "tofu",
        CertificatePolicy::System => "system",
        CertificatePolicy::Ignore => "ignore",
        CertificatePolicy::Deny => "deny",
    }
}

const fn next_policy(policy: CertificatePolicy) -> CertificatePolicy {
    match policy {
        CertificatePolicy::Tofu => CertificatePolicy::System,
        CertificatePolicy::System => CertificatePolicy::Ignore,
        CertificatePolicy::Ignore => CertificatePolicy::Deny,
        CertificatePolicy::Deny => CertificatePolicy::Tofu,
    }
}

#[cfg(test)]
mod tests {
    use super::{App, EditForm, Mode, PromptAction};
    use crate::config::ConfigStore;
    use crate::model::{
        CertificatePolicy, DeviceConfig, DisplayConfig, Endpoint, IdentityConfig, Profile,
        ProfileId, Renderer, Route, SecurityConfig,
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

    fn app_on_disk(names: &[&str]) -> (TempDir, App) {
        let dir = TempDir::new().unwrap();
        let config_root = dir.path().to_path_buf();
        let store = ProfileStore::new(ConfigStore::new(config_root.as_path()));
        let profiles: Vec<Profile> = names
            .iter()
            .map(|name| {
                let mut profile = sample_profile();
                profile.name = (*name).to_string();
                store.upsert(profile.clone()).unwrap();
                profile
            })
            .collect();
        let app = App::new(profiles, PathBuf::from("/usr/bin/rdp-tui"), config_root);
        (dir, app)
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
    fn renders_add_hint_with_no_profiles() {
        let mut app = app_with(&[]);
        assert!(rendered(&mut app, 80, 24).contains("add"));
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

    fn type_text(app: &mut App, text: &str) {
        for character in text.chars() {
            app.handle_editing(press(KeyCode::Char(character)));
        }
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
            config_root,
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

    #[test]
    fn adding_a_profile_through_the_form_persists_it() {
        let (dir, mut app) = app_on_disk(&[]);
        app.handle_browsing(press(KeyCode::Char('a')));
        assert!(matches!(app.mode, Mode::Editing(_)));

        // Name field.
        app.handle_editing(press(KeyCode::Enter));
        type_text(&mut app, "Workbench");
        app.handle_editing(press(KeyCode::Enter));
        // Move to Host and type it.
        app.handle_editing(press(KeyCode::Down));
        app.handle_editing(press(KeyCode::Enter));
        type_text(&mut app, "10.0.0.9:3390");
        app.handle_editing(press(KeyCode::Enter));
        // Accept.
        app.handle_editing(press(KeyCode::Char('A')));

        assert!(matches!(app.mode, Mode::Browsing));
        let store = ProfileStore::new(ConfigStore::new(dir.path()));
        let saved = store.list().unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].name, "Workbench");
        assert_eq!(saved[0].endpoint.to_string(), "10.0.0.9:3390");
    }

    #[test]
    fn editing_a_profile_cycles_certificate_and_persists() {
        let (dir, mut app) = app_on_disk(&["Sample"]);
        let id = app.profiles[0].id;
        app.handle_browsing(press(KeyCode::Char('e')));
        // Field 6 is Certificate; move down to it and cycle with Space.
        for _ in 0..6 {
            app.handle_editing(press(KeyCode::Down));
        }
        app.handle_editing(press(KeyCode::Char(' ')));
        app.handle_editing(press(KeyCode::Char('A')));

        let store = ProfileStore::new(ConfigStore::new(dir.path()));
        let saved = store.get(id).unwrap().unwrap();
        // Default Tofu advances one step to System.
        assert_eq!(saved.security.certificate_policy, CertificatePolicy::System);
    }

    #[test]
    fn cloning_duplicates_the_selected_profile_without_its_credential() {
        let (dir, mut app) = app_on_disk(&["Sample"]);
        // Give the source a saved password so we can prove the clone drops it.
        app.begin_password();
        for character in "hunter2".chars() {
            app.handle_password(press(KeyCode::Char(character)));
        }
        app.handle_password(press(KeyCode::Enter));

        app.handle_browsing(press(KeyCode::Char('c')));

        let store = ProfileStore::new(ConfigStore::new(dir.path()));
        let all = store.list().unwrap();
        assert_eq!(all.len(), 2);
        let copy = all
            .iter()
            .find(|profile| profile.name.ends_with("(copy)"))
            .unwrap();
        assert!(copy.credential.is_none());
    }

    #[test]
    fn deleting_a_confirmed_profile_removes_it() {
        let (dir, mut app) = app_on_disk(&["Sample"]);
        let id = app.profiles[0].id;
        app.handle_browsing(press(KeyCode::Char('d')));
        assert!(matches!(
            app.mode,
            Mode::Prompt {
                action: PromptAction::ConfirmDelete(_),
                ..
            }
        ));
        for character in "yes".chars() {
            app.handle_prompt(press(KeyCode::Char(character)));
        }
        app.handle_prompt(press(KeyCode::Enter));

        let store = ProfileStore::new(ConfigStore::new(dir.path()));
        assert!(store.get(id).unwrap().is_none());
        assert!(app.profiles.is_empty());
    }

    #[test]
    fn find_filters_the_visible_profiles() {
        let mut app = app_with(&["Anima", "Compono", "Basalt"]);
        app.handle_browsing(press(KeyCode::Char('f')));
        for character in "compo".chars() {
            app.handle_prompt(press(KeyCode::Char(character)));
        }
        app.handle_prompt(press(KeyCode::Enter));

        assert_eq!(app.visible.len(), 1);
        assert_eq!(app.current().unwrap().name, "Compono");
        let screen = rendered(&mut app, 80, 24);
        assert!(screen.contains("Compono"));
        assert!(!screen.contains("Anima"));
    }

    #[test]
    fn exporting_writes_an_rdp_file_without_a_password() {
        let (_dir, mut app) = app_on_disk(&["Anima"]);
        let out = TempDir::new().unwrap();
        let path = out.path().join("anima"); // extension is added for us
        let id = app.profiles[0].id;
        app.perform_export(id, path.to_str().unwrap());

        let written = std::fs::read_to_string(path.with_extension("rdp")).unwrap();
        assert!(written.contains("full address:s:10.0.0.5:3389"));
        assert!(!written.to_lowercase().contains("password"));
    }

    #[test]
    fn importing_an_exported_profile_adds_it_then_dedups_on_reimport() {
        let (_source, mut app) = app_on_disk(&["Anima"]);
        let out = TempDir::new().unwrap();
        let path = out.path().join("anima.rdp");
        let id = app.profiles[0].id;
        app.perform_export(id, path.to_str().unwrap());

        let (_fresh_dir, mut fresh) = app_on_disk(&[]);
        fresh.perform_import(path.to_str().unwrap());
        assert_eq!(fresh.profiles.len(), 1);

        // A second import of the same file changes nothing.
        fresh.perform_import(path.to_str().unwrap());
        assert_eq!(fresh.profiles.len(), 1);
        assert!(fresh.status.contains("skipped 1"));
    }

    #[test]
    fn status_overlay_shows_the_selected_profile() {
        let mut app = app_with(&["Anima"]);
        app.handle_browsing(press(KeyCode::Char('s')));
        assert!(matches!(app.mode, Mode::Status(_)));
        let screen = rendered(&mut app, 80, 24);
        assert!(screen.contains("Anima"));
        assert!(screen.contains("Active sessions"));
    }

    #[test]
    fn from_profile_seeds_the_edit_form() {
        let mut profile = sample_profile();
        profile.name = "Anima".into();
        profile.identity.username = "operator".into();
        let form = EditForm::from_profile(&profile);
        assert_eq!(form.name, "Anima");
        assert_eq!(form.username, "operator");
        assert_eq!(form.renderer, Renderer::WaylandSdl);
        assert!(form.target.is_some());
    }
}
