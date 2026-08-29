//! Ratatui frontend: a profile list that connects, tests, stores passwords, and
//! manages profiles (add/edit/clone/delete/find/import/export/status) through the
//! same `session`/`credentials`/`profile_store` functions the CLI uses (INV-8).
//! The edit form edits every field the CLI `set` command does, sharing one
//! parser/formatter implementation in `model::fields` (the tui<->cli edge is
//! forbidden, so nothing is imported from `cli`).

use super::terminal::TerminalGuard;
use crate::config::ConfigStore;
use crate::credentials::{SystemCredentialStore, forget_encrypted, store_encrypted_password};
use crate::model::fields;
use crate::model::{
    DeviceConfig, DisplayConfig, Endpoint, IdentityConfig, Profile, ProfileId, Route,
    SecurityConfig,
};
use crate::profile_store::ProfileStore;
use crate::session::{DEEP_TEST_WARNING, connect_profile, test_profile};
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
    ConfirmDeepTest(ProfileId),
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
    Editing(Box<EditForm>),
    /// A read-only details/session overlay; any key returns to browsing.
    Status(String),
}

/// Every editable profile field, in the order shown in the form. Matches the set
/// the CLI `set` command accepts, so the two frontends have parity.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    Name,
    Host,
    Username,
    Domain,
    Route,
    Renderer,
    Fullscreen,
    Resolution,
    Scale,
    ColorDepth,
    DynamicResolution,
    Multimon,
    SpanMonitors,
    SmartSizing,
    Certificate,
    Clipboard,
    Audio,
    Microphone,
    Printers,
}

const FIELDS: [Field; 19] = [
    Field::Name,
    Field::Host,
    Field::Username,
    Field::Domain,
    Field::Route,
    Field::Renderer,
    Field::Fullscreen,
    Field::Resolution,
    Field::Scale,
    Field::ColorDepth,
    Field::DynamicResolution,
    Field::Multimon,
    Field::SpanMonitors,
    Field::SmartSizing,
    Field::Certificate,
    Field::Clipboard,
    Field::Audio,
    Field::Microphone,
    Field::Printers,
];

impl Field {
    const fn label(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Host => "Host",
            Self::Username => "Username",
            Self::Domain => "Domain",
            Self::Route => "Route",
            Self::Renderer => "Renderer",
            Self::Fullscreen => "Fullscreen",
            Self::Resolution => "Resolution",
            Self::Scale => "Scale",
            Self::ColorDepth => "Color depth",
            Self::DynamicResolution => "Dynamic res",
            Self::Multimon => "Multi-monitor",
            Self::SpanMonitors => "Span monitors",
            Self::SmartSizing => "Smart sizing",
            Self::Certificate => "Certificate",
            Self::Clipboard => "Clipboard",
            Self::Audio => "Audio",
            Self::Microphone => "Microphone",
            Self::Printers => "Printers",
        }
    }

    /// Text fields are typed into; the rest are cycled or toggled in place.
    const fn is_text(self) -> bool {
        matches!(
            self,
            Self::Name
                | Self::Host
                | Self::Username
                | Self::Domain
                | Self::Route
                | Self::Resolution
        )
    }
}

/// A working copy of a profile being added (`target` is `None`) or edited. The
/// draft holds every field except the host, which is kept as text and parsed
/// into an `Endpoint` on save.
#[derive(Clone)]
struct EditForm {
    target: Option<ProfileId>,
    draft: Profile,
    host: String,
    field: usize,
    /// `Some` while the highlighted text field is being typed into.
    editing_text: Option<String>,
    /// The last parse error to show under the form.
    error: Option<String>,
}

impl EditForm {
    fn blank() -> Self {
        Self {
            target: None,
            draft: blank_profile(),
            host: String::new(),
            field: 0,
            editing_text: None,
            error: None,
        }
    }

    fn from_profile(profile: &Profile) -> Self {
        Self {
            target: Some(profile.id),
            draft: profile.clone(),
            host: profile.endpoint.to_string(),
            field: 0,
            editing_text: None,
            error: None,
        }
    }

    fn move_field(&mut self, forward: bool) {
        let count = FIELDS.len();
        self.field = if forward {
            (self.field + 1) % count
        } else {
            (self.field + count - 1) % count
        };
        self.error = None;
    }

    fn display_value(&self, field: Field) -> String {
        match field {
            Field::Name => self.draft.name.clone(),
            Field::Host => self.host.clone(),
            Field::Username => self.draft.identity.username.clone(),
            Field::Domain => self.draft.identity.domain.clone(),
            Field::Route => self.draft.route.to_token(),
            Field::Renderer => self.draft.display.renderer.token().to_owned(),
            Field::Fullscreen => fields::format_bool(self.draft.display.fullscreen).to_owned(),
            Field::Resolution => fields::format_resolution(self.draft.display.resolution),
            Field::Scale => fields::format_scale(self.draft.display.scale_percent),
            Field::ColorDepth => fields::format_color_depth(self.draft.display.color_depth),
            Field::DynamicResolution => {
                fields::format_bool(self.draft.display.dynamic_resolution).to_owned()
            }
            Field::Multimon => fields::format_bool(self.draft.display.multimon).to_owned(),
            Field::SpanMonitors => fields::format_bool(self.draft.display.span_monitors).to_owned(),
            Field::SmartSizing => fields::format_bool(self.draft.display.smart_sizing).to_owned(),
            Field::Certificate => self.draft.security.certificate_policy.token().to_owned(),
            Field::Clipboard => fields::format_bool(self.draft.devices.clipboard).to_owned(),
            Field::Audio => fields::format_bool(self.draft.devices.audio_playback).to_owned(),
            Field::Microphone => fields::format_bool(self.draft.devices.microphone).to_owned(),
            Field::Printers => fields::format_bool(self.draft.devices.printers).to_owned(),
        }
    }

    /// The editable text for a text field (what pressing Enter loads).
    fn text_value(&self, field: Field) -> String {
        match field {
            Field::Route => self.draft.route.to_token(),
            Field::Resolution => fields::format_resolution(self.draft.display.resolution),
            _ => self.display_value(field),
        }
    }

    /// Commit a typed buffer into the draft. On a parse error, keep the buffer
    /// open and record the message so the user can fix it.
    fn commit_text(&mut self, field: Field, text: String) {
        match field {
            Field::Name => self.draft.name = text,
            Field::Host => self.host = text,
            Field::Username => self.draft.identity.username = text,
            Field::Domain => self.draft.identity.domain = text,
            Field::Route => match Route::from_token(text.trim()) {
                Ok(route) => self.draft.route = route,
                Err(error) => {
                    self.error = Some(error);
                    self.editing_text = Some(text);
                    return;
                }
            },
            Field::Resolution => match fields::parse_resolution(text.trim()) {
                Ok(resolution) => self.draft.display.resolution = resolution,
                Err(error) => {
                    self.error = Some(error);
                    self.editing_text = Some(text);
                    return;
                }
            },
            _ => {}
        }
        self.error = None;
        self.editing_text = None;
    }

    /// Cycle an enum field or toggle a boolean field. A no-op on text fields.
    fn cycle(&mut self, field: Field) {
        let display = &mut self.draft.display;
        match field {
            Field::Renderer => display.renderer = display.renderer.cycled(),
            Field::Scale => display.scale_percent = fields::cycle_scale(display.scale_percent),
            Field::ColorDepth => {
                display.color_depth = fields::cycle_color_depth(display.color_depth);
            }
            Field::Certificate => {
                let policy = &mut self.draft.security.certificate_policy;
                *policy = policy.cycled();
            }
            Field::Fullscreen => toggle(&mut display.fullscreen),
            Field::DynamicResolution => toggle(&mut display.dynamic_resolution),
            Field::Multimon => toggle(&mut display.multimon),
            Field::SpanMonitors => toggle(&mut display.span_monitors),
            Field::SmartSizing => toggle(&mut display.smart_sizing),
            Field::Clipboard => toggle(&mut self.draft.devices.clipboard),
            Field::Audio => toggle(&mut self.draft.devices.audio_playback),
            Field::Microphone => toggle(&mut self.draft.devices.microphone),
            Field::Printers => toggle(&mut self.draft.devices.printers),
            // Text fields are edited by typing, never cycled.
            Field::Name
            | Field::Host
            | Field::Username
            | Field::Domain
            | Field::Route
            | Field::Resolution => {}
        }
    }
}

fn toggle(flag: &mut bool) {
    *flag = !*flag;
}

fn deep_test_message(outcome: crate::session::DeepTest) -> &'static str {
    use crate::session::DeepTest;
    match outcome {
        DeepTest::Authenticated => "credentials accepted",
        DeepTest::AuthFailed => "authentication failed — the host rejected the credentials",
        DeepTest::Unreachable => "could not reach the host to authenticate",
        DeepTest::NotSupported => "auth-only is not supported by this FreeRDP build",
        DeepTest::RateLimited => "skipped — deep-tested too recently (try again shortly)",
        DeepTest::NeedsAcknowledgement => "confirmation required",
    }
}

fn state_dir() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .unwrap_or_else(|| PathBuf::from(".local/state"))
        .join("rdp-tui")
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
            KeyCode::Char('D') => self.deep_test(),
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
            PromptAction::ConfirmDeepTest(id) => {
                if input.trim().eq_ignore_ascii_case("yes") {
                    if let Some(profile) = self.profiles.iter().find(|p| p.id == id).cloned() {
                        self.run_deep_test(&profile, true);
                    }
                } else {
                    self.status = "Deep-test cancelled.".into();
                }
            }
            PromptAction::Import => self.perform_import(input.trim()),
            PromptAction::Export(id) => self.perform_export(id, input.trim()),
        }
    }

    fn handle_editing(&mut self, key: KeyEvent) {
        // Typing into a text field: characters, Backspace, Enter (commit), Esc (abort).
        if let Mode::Editing(form) = &mut self.mode
            && form.editing_text.is_some()
        {
            match key.code {
                KeyCode::Esc => {
                    form.editing_text = None;
                    form.error = None;
                }
                KeyCode::Enter => {
                    let field = FIELDS[form.field];
                    if let Some(text) = form.editing_text.take() {
                        form.commit_text(field, text);
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
            KeyCode::Enter | KeyCode::Char(' ' | 'e') => self.activate_field(),
            KeyCode::Char('a' | 'A') => self.save_form(),
            _ => {}
        }
    }

    /// Enter/Space on a field: begin typing a text field, or cycle/toggle it.
    fn activate_field(&mut self) {
        if let Mode::Editing(form) = &mut self.mode {
            let field = FIELDS[form.field];
            form.error = None;
            if field.is_text() {
                form.editing_text = Some(form.text_value(field));
            } else {
                form.cycle(field);
            }
        }
    }

    fn save_form(&mut self) {
        let form = match &self.mode {
            Mode::Editing(form) => form.clone(),
            _ => return,
        };
        if form.draft.name.trim().is_empty() {
            self.set_form_error("A name is required.");
            return;
        }
        let endpoint = match form.host.trim().parse::<Endpoint>() {
            Ok(endpoint) => endpoint,
            Err(error) => {
                self.set_form_error(&format!("Invalid host: {error}"));
                return;
            }
        };
        let mut profile = form.draft.clone();
        profile.name = profile.name.trim().to_string();
        profile.endpoint = endpoint;
        let id = profile.id;
        let name = profile.name.clone();
        let adding = form.target.is_none();
        match self.store().upsert(profile) {
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
            // The store validates on write (e.g. an impossible multimon+span
            // combination); surface that under the still-open form.
            Err(error) => self.set_form_error(&error.to_string()),
        }
    }

    fn set_form_error(&mut self, message: &str) {
        if let Mode::Editing(form) = &mut self.mode {
            form.error = Some(message.to_owned());
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
                    profile.security.certificate_policy.token(),
                )
            }
            None if self.profiles.is_empty() => "No profiles yet. Press a to add one.".to_string(),
            None => format!("No matches for filter: {}", self.filter),
        }
    }

    fn begin_add(&mut self) {
        self.mode = Mode::Editing(Box::new(EditForm::blank()));
        self.status = "Adding a profile — A to accept, Esc to cancel.".into();
    }

    fn begin_edit(&mut self) {
        let Some(profile) = self.current() else {
            self.status = "No profile to edit.".into();
            return;
        };
        let name = profile.name.clone();
        self.mode = Mode::Editing(Box::new(EditForm::from_profile(profile)));
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
            let _ = writeln!(text, "Route: {}", profile.route.to_token());
            let _ = writeln!(
                text,
                "Renderer: {}   Fullscreen: {}",
                profile.display.renderer.token(),
                fields::format_bool(profile.display.fullscreen)
            );
            let _ = writeln!(
                text,
                "Certificate: {}   Password: {}",
                profile.security.certificate_policy.token(),
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

    fn deep_test(&mut self) {
        let Some(profile) = self.current().cloned() else {
            return;
        };
        self.run_deep_test(&profile, false);
    }

    fn run_deep_test(&mut self, profile: &Profile, acknowledged: bool) {
        let store = SystemCredentialStore::new(self.config_root.as_path());
        match crate::session::deep_test_profile(profile, &store, &state_dir(), acknowledged) {
            // First deep-test of this profile: confirm the one-time warning.
            Ok(crate::session::DeepTest::NeedsAcknowledgement) => {
                self.mode = Mode::Prompt {
                    label: format!("{} Deep-test {}? type yes", DEEP_TEST_WARNING, profile.name),
                    input: String::new(),
                    action: PromptAction::ConfirmDeepTest(profile.id),
                };
            }
            Ok(outcome) => {
                self.status = format!("{}: {}", profile.name, deep_test_message(outcome));
            }
            Err(error) => self.status = format!("{}: deep-test failed: {error}", profile.name),
        }
    }

    fn begin_password(&mut self) {
        let Some(name) = self.current().map(|profile| profile.name.clone()) else {
            return;
        };
        self.mode = Mode::Password(String::new());
        self.status =
            format!("Password for {name} — Enter to save, empty Enter to clear, Esc to cancel");
    }

    fn commit_password(&mut self) {
        let password = match &self.mode {
            Mode::Password(input) => input.clone(),
            _ => return,
        };
        self.mode = Mode::Browsing;
        let Some(index) = self.current_index() else {
            return;
        };
        let name = self.profiles[index].name.clone();
        if password.is_empty() {
            // Empty = clear any saved password (parity with `credential clear`).
            self.status = match self.clear_password(index) {
                Ok(true) => format!("Cleared the password for {name}"),
                Ok(false) => format!("{name} had no saved password"),
                Err(error) => format!("Could not clear password: {error}"),
            };
            return;
        }
        self.status = match self.save_password(index, &password) {
            Ok(()) => format!("Saved a password for {name}"),
            Err(error) => format!("Could not save password: {error}"),
        };
    }

    fn clear_password(&mut self, index: usize) -> Result<bool, String> {
        let mut profile = self.profiles[index].clone();
        let Some(reference) = profile.credential.take() else {
            return Ok(false);
        };
        self.store()
            .upsert(profile.clone())
            .map_err(|error| error.to_string())?;
        self.profiles[index] = profile;
        forget_encrypted(self.config_root.as_path(), reference);
        Ok(true)
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
            // A local ListState keeps the highlighted field scrolled into view.
            let mut state = ListState::default();
            state.select(Some(form.field));
            frame.render_stateful_widget(list, areas[0], &mut state);
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
                " Enter connect · a/e add/edit · c clone · d delete · f find · i/x import/export · s status · t test · D deep-test · p pass · q quit ".to_string(),
                self.status.clone(),
            ),
            Mode::Password(input) => (
                " typing password · Enter save · Esc cancel ".to_string(),
                format!("Password: {}", "*".repeat(input.chars().count())),
            ),
            Mode::Prompt { label, input, .. } => (
                " Enter confirm · Esc cancel ".to_string(),
                format!("{label}: {input}"),
            ),
            Mode::Status(_) => (" any key to return ".to_string(), String::new()),
            Mode::Editing(form) => (
                if form.editing_text.is_some() {
                    " typing · Enter set · Esc cancel field ".to_string()
                } else {
                    " ↑/↓ field · Enter/Space edit · A accept · Esc cancel ".to_string()
                },
                form.error.clone().unwrap_or_else(|| self.status.clone()),
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

#[cfg(test)]
mod tests {
    use super::{App, EditForm, FIELDS, Field, Mode, PromptAction};
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

    // Opening a text field loads its current value; clear it, then type anew.
    fn replace_text(app: &mut App, text: &str) {
        loop {
            let empty = match &app.mode {
                Mode::Editing(form) => form.editing_text.as_deref().is_none_or(str::is_empty),
                _ => true,
            };
            if empty {
                break;
            }
            app.handle_editing(press(KeyCode::Backspace));
        }
        type_text(app, text);
    }

    // Move the form cursor to `field` regardless of where it currently sits
    // (Up/Down wrap, so counting from zero is not reliable).
    fn go_to_field(app: &mut App, field: Field) {
        let target = FIELDS.iter().position(|current| *current == field).unwrap();
        loop {
            let current = match &app.mode {
                Mode::Editing(form) => form.field,
                _ => panic!("not in the edit form"),
            };
            if current == target {
                break;
            }
            app.handle_editing(press(KeyCode::Down));
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
    fn an_empty_password_clears_a_saved_credential() {
        let (dir, mut app) = app_on_disk(&["Sample"]);
        let id = app.profiles[0].id;

        app.begin_password();
        for character in "hunter2".chars() {
            app.handle_password(press(KeyCode::Char(character)));
        }
        app.handle_password(press(KeyCode::Enter));
        assert!(app.profiles[0].credential.is_some());

        // Re-open and submit nothing: the saved password is cleared.
        app.begin_password();
        app.handle_password(press(KeyCode::Enter));
        assert!(app.profiles[0].credential.is_none());

        let store = ProfileStore::new(ConfigStore::new(dir.path()));
        assert!(store.get(id).unwrap().unwrap().credential.is_none());
    }

    #[test]
    fn adding_a_profile_through_the_form_persists_it() {
        let (dir, mut app) = app_on_disk(&[]);
        app.handle_browsing(press(KeyCode::Char('a')));
        assert!(matches!(app.mode, Mode::Editing(_)));

        // Name field (index 0).
        app.handle_editing(press(KeyCode::Enter));
        type_text(&mut app, "Workbench");
        app.handle_editing(press(KeyCode::Enter));
        // Host field.
        go_to_field(&mut app, Field::Host);
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
    fn editing_advanced_fields_through_the_form_persists_them() {
        let (dir, mut app) = app_on_disk(&["Sample"]);
        let id = app.profiles[0].id;
        app.handle_browsing(press(KeyCode::Char('e')));

        // Cycle the certificate policy (Tofu -> System).
        go_to_field(&mut app, Field::Certificate);
        app.handle_editing(press(KeyCode::Char(' ')));
        // Toggle multi-monitor on.
        go_to_field(&mut app, Field::Multimon);
        app.handle_editing(press(KeyCode::Char(' ')));
        // Set the route by typing (clearing the default "direct" first).
        go_to_field(&mut app, Field::Route);
        app.handle_editing(press(KeyCode::Enter));
        replace_text(&mut app, "ssh:jump.example");
        app.handle_editing(press(KeyCode::Enter));
        app.handle_editing(press(KeyCode::Char('A')));

        let store = ProfileStore::new(ConfigStore::new(dir.path()));
        let saved = store.get(id).unwrap().unwrap();
        assert_eq!(saved.security.certificate_policy, CertificatePolicy::System);
        assert!(saved.display.multimon);
        assert!(matches!(saved.route, Route::SshTunnel { .. }));
    }

    #[test]
    fn an_invalid_host_keeps_the_form_open_with_an_error() {
        let (_dir, mut app) = app_on_disk(&[]);
        app.begin_add();
        app.handle_editing(press(KeyCode::Enter));
        type_text(&mut app, "Named");
        app.handle_editing(press(KeyCode::Enter));
        // Accept without ever setting a host.
        app.handle_editing(press(KeyCode::Char('A')));
        assert!(matches!(app.mode, Mode::Editing(_)));
        if let Mode::Editing(form) = &app.mode {
            assert!(form.error.is_some());
        }
    }

    #[test]
    fn cloning_duplicates_the_selected_profile_without_its_credential() {
        let (dir, mut app) = app_on_disk(&["Sample"]);
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
    fn deep_test_asks_for_confirmation_on_first_use() {
        // A fresh profile has no deep-test stamp, so `D` must confirm the
        // one-time warning before anything runs.
        let mut app = app_with(&["Anima"]);
        app.handle_browsing(press(KeyCode::Char('D')));
        assert!(matches!(
            app.mode,
            Mode::Prompt {
                action: PromptAction::ConfirmDeepTest(_),
                ..
            }
        ));
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
        assert_eq!(form.draft.name, "Anima");
        assert_eq!(form.draft.identity.username, "operator");
        assert_eq!(form.draft.display.renderer, Renderer::WaylandSdl);
        assert!(form.target.is_some());
    }
}
