//! Ratatui frontend: a profile list that connects, tests, stores passwords, and
//! manages profiles (add/edit/clone/delete/find/import/export/status) through the
//! same `session`/`credentials`/`profile_store` functions the CLI uses (INV-8).
//! The edit form edits every field the CLI `set` command does, sharing one
//! parser/formatter implementation in `model::fields` (the tui<->cli edge is
//! forbidden, so nothing is imported from `cli`).

use super::terminal::TerminalGuard;
mod view;
use crate::config::ConfigStore;
use crate::credentials::SystemCredentialStore;
use crate::model::fields;
use crate::model::{
    DeviceConfig, DisplayConfig, Endpoint, HistoryEntry, IdentityConfig, MessageEntry, Profile,
    ProfileId, Route, SecurityConfig, SessionId,
};
use crate::operations::{self, CredentialSlot};
use crate::profile_store::ProfileStore;
use crate::session::{
    DEEP_TEST_WARNING, connect_profile, deep_test_requires_acknowledgement, test_profile,
};
use ratatui::Terminal;
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::widgets::ListState;
use secrecy::zeroize::Zeroizing;
use std::collections::VecDeque;
use std::fmt::Write as _;
use std::io;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

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
    app.load_message_history(&state_dir());
    app.watch_existing_sessions();

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
    Password {
        slot: CredentialSlot,
        input: Zeroizing<String>,
    },
    Prompt {
        label: String,
        input: String,
        action: PromptAction,
    },
    Editing(Box<EditForm>),
    /// Read-only, scrollable details; explicit dismissal returns to browsing.
    Status(TextView),
    CertificateMismatch(crate::freerdp::certificate::CertificateMismatch),
}

/// A modal message; scrolling bounds are refreshed when the terminal is resized.
struct TextView {
    title: &'static str,
    message: String,
    scroll: u16,
    max_scroll: u16,
    page_height: u16,
}

impl TextView {
    fn new(title: &'static str, mut message: String) -> Self {
        message.truncate(message.trim_end().len());
        Self {
            title,
            message,
            scroll: 0,
            max_scroll: 0,
            page_height: 1,
        }
    }

    /// Returns true only for an explicit dismissal key.
    fn handle_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => {
                self.scroll = self.scroll.saturating_add(1).min(self.max_scroll);
            }
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(self.page_height),
            KeyCode::PageDown => {
                self.scroll = self
                    .scroll
                    .saturating_add(self.page_height)
                    .min(self.max_scroll);
            }
            KeyCode::Home => self.scroll = 0,
            KeyCode::End => self.scroll = self.max_scroll,
            KeyCode::Enter | KeyCode::Esc | KeyCode::Char('q') => return true,
            _ => {}
        }
        false
    }
}

struct WatchedSession {
    id: SessionId,
    name: String,
    initiated_here: bool,
}

enum BackgroundResult {
    Status(String),
    Error(String),
}

/// Every editable profile field, in the order shown in the form. Matches the set
/// the CLI `set` command accepts, so the two frontends have parity.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    Name,
    Host,
    Username,
    Domain,
    GatewayUsername,
    GatewayDomain,
    Route,
    Renderer,
    Fullscreen,
    Resolution,
    Scale,
    ColorDepth,
    Graphics,
    DynamicResolution,
    Multimon,
    SpanMonitors,
    SmartSizing,
    Certificate,
    AdminSession,
    Network,
    Clipboard,
    Audio,
    Microphone,
    Printers,
}

const FIELDS: [Field; 24] = [
    Field::Name,
    Field::Host,
    Field::Username,
    Field::Domain,
    Field::GatewayUsername,
    Field::GatewayDomain,
    Field::Route,
    Field::Renderer,
    Field::Fullscreen,
    Field::Resolution,
    Field::Scale,
    Field::ColorDepth,
    Field::Graphics,
    Field::DynamicResolution,
    Field::Multimon,
    Field::SpanMonitors,
    Field::SmartSizing,
    Field::Certificate,
    Field::AdminSession,
    Field::Network,
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
            Self::GatewayUsername => "Gateway user",
            Self::GatewayDomain => "Gateway domain",
            Self::Route => "Route",
            Self::Renderer => "Renderer",
            Self::Fullscreen => "Fullscreen",
            Self::Resolution => "Resolution",
            Self::Scale => "Scale",
            Self::ColorDepth => "Color depth",
            Self::Graphics => "Graphics",
            Self::DynamicResolution => "Dynamic res",
            Self::Multimon => "Multi-monitor",
            Self::SpanMonitors => "Span monitors",
            Self::SmartSizing => "Smart sizing",
            Self::Certificate => "Certificate",
            Self::AdminSession => "Admin session",
            Self::Network => "Network",
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
                | Self::GatewayUsername
                | Self::GatewayDomain
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
            Field::GatewayUsername => match &self.draft.route {
                Route::RdGateway { username, .. } => username.clone(),
                _ => "(set gateway route first)".into(),
            },
            Field::GatewayDomain => match &self.draft.route {
                Route::RdGateway { domain, .. } => domain.clone(),
                _ => "(set gateway route first)".into(),
            },
            Field::Route => self.draft.route.to_token(),
            Field::Renderer => self.draft.display.renderer.token().to_owned(),
            Field::Fullscreen => fields::format_bool(self.draft.display.fullscreen).to_owned(),
            Field::Resolution => fields::format_resolution(self.draft.display.resolution),
            Field::Scale => fields::format_scale(self.draft.display.scale_percent),
            Field::ColorDepth => fields::format_color_depth(self.draft.display.color_depth),
            Field::Graphics => self.draft.display.graphics.token().to_owned(),
            Field::DynamicResolution => {
                fields::format_bool(self.draft.display.dynamic_resolution).to_owned()
            }
            Field::Multimon => fields::format_bool(self.draft.display.multimon).to_owned(),
            Field::SpanMonitors => fields::format_bool(self.draft.display.span_monitors).to_owned(),
            Field::SmartSizing => fields::format_bool(self.draft.display.smart_sizing).to_owned(),
            Field::Certificate => self.draft.security.certificate_policy.token().to_owned(),
            Field::AdminSession => {
                fields::format_bool(self.draft.security.admin_session).to_owned()
            }
            Field::Network => self.draft.security.network_profile.token().to_owned(),
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
            Field::GatewayUsername => {
                let Route::RdGateway { username, .. } = &mut self.draft.route else {
                    self.error = Some("Set Route to gateway:<host> first".into());
                    return;
                };
                *username = text;
            }
            Field::GatewayDomain => {
                let Route::RdGateway { domain, .. } = &mut self.draft.route else {
                    self.error = Some("Set Route to gateway:<host> first".into());
                    return;
                };
                *domain = text;
            }
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
            Field::Graphics => display.graphics = display.graphics.cycled(),
            Field::Certificate => {
                let policy = &mut self.draft.security.certificate_policy;
                *policy = policy.cycled();
            }
            Field::AdminSession => toggle(&mut self.draft.security.admin_session),
            Field::Network => {
                let network = &mut self.draft.security.network_profile;
                *network = network.cycled();
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
            | Field::GatewayUsername
            | Field::GatewayDomain
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
        DeepTest::Indeterminate => "FreeRDP returned no recognized authentication result",
        DeepTest::NotSupported => "auth-only is not supported by this FreeRDP build",
        DeepTest::RateLimited => "skipped — deep-tested too recently (try again shortly)",
        DeepTest::NeedsAcknowledgement => "confirmation required",
    }
}

fn session_message(entry: &HistoryEntry, name: &str) -> String {
    let mut message = entry.summarize(name, now_secs());
    if entry.failure == Some(crate::model::ConnectionFailure::Authentication) {
        message.push_str("\n\nSelect this profile and press p to update its saved password. Use e to check the username and domain.");
    } else if entry.failure == Some(crate::model::ConnectionFailure::Certificate) {
        message.push_str("\n\nSelect this profile and press T to review the changed certificate before reconnecting.");
    } else if entry.failure.is_some() {
        message.push_str("\n\nUse t to check reachability or D to test authentication. Use L to enable diagnostics before reproducing the failure.");
    }
    message
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

fn state_dir() -> PathBuf {
    crate::paths::state_root()
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
    background_tx: Sender<BackgroundResult>,
    background_rx: Receiver<BackgroundResult>,
    background_busy: bool,
    launched_sessions: Vec<WatchedSession>,
    last_session_poll: Instant,
    error_dialog: Option<TextView>,
    pending_errors: VecDeque<String>,
    messages: Vec<MessageEntry>,
    message_store: Option<ConfigStore>,
    notification_pending: bool,
}

impl App {
    fn new(profiles: Vec<Profile>, executable: PathBuf, config_root: PathBuf) -> Self {
        let (background_tx, background_rx) = mpsc::channel();
        let mut app = Self {
            profiles,
            visible: Vec::new(),
            filter: String::new(),
            selected: ListState::default(),
            status: String::new(),
            executable,
            config_root,
            mode: Mode::Browsing,
            background_tx,
            background_rx,
            background_busy: false,
            launched_sessions: Vec::new(),
            last_session_poll: Instant::now(),
            error_dialog: None,
            pending_errors: VecDeque::new(),
            messages: Vec::new(),
            message_store: None,
            notification_pending: false,
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
            Err(error) => self.queue_error(format!("Could not reload profiles: {error}")),
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

    fn event_loop<B: Backend<Error = io::Error>>(
        &mut self,
        terminal: &mut Terminal<B>,
    ) -> io::Result<()> {
        loop {
            self.receive_background_result();
            if self.last_session_poll.elapsed() >= Duration::from_millis(250) {
                self.receive_session_results(&state_dir());
                self.last_session_poll = Instant::now();
            }
            self.present_pending_error();
            terminal.draw(|frame| self.draw(frame))?;
            if !event::poll(Duration::from_millis(50))? {
                continue;
            }
            let Event::Key(key) = event::read()? else {
                continue;
            };
            if key.kind != KeyEventKind::Press {
                continue;
            }
            if self.error_dialog.is_some() {
                self.handle_error_dialog(key);
                continue;
            }
            match &self.mode {
                Mode::Browsing => {
                    if self.handle_browsing(key) {
                        return Ok(());
                    }
                }
                Mode::Password { .. } => self.handle_password(key),
                Mode::Prompt { .. } => self.handle_prompt(key),
                Mode::Editing(_) => self.handle_editing(key),
                Mode::Status(_) => self.handle_status(key),
                Mode::CertificateMismatch(_) => self.handle_certificate_mismatch(key),
            }
        }
    }

    fn receive_background_result(&mut self) {
        while let Ok(result) = self.background_rx.try_recv() {
            self.background_busy = false;
            match result {
                BackgroundResult::Status(status) => self.status = status,
                BackgroundResult::Error(message) => self.queue_error(message),
            }
        }
    }

    fn receive_session_results(&mut self, state_root: &Path) {
        if self.launched_sessions.is_empty() {
            return;
        }
        let Ok(history) = ConfigStore::new(state_root).load_history() else {
            return;
        };
        let mut notifications = Vec::new();
        self.launched_sessions.retain(|watched| {
            let Some(entry) = history
                .entries
                .iter()
                .find(|entry| entry.session_id == Some(watched.id))
            else {
                return true;
            };
            let message = session_message(entry, &watched.name);
            notifications.push((
                message,
                !entry.succeeded(),
                watched.initiated_here && !entry.established,
                watched.id,
                watched.name.clone(),
            ));
            false
        });
        for (message, failed, action_failed, session, name) in notifications {
            if failed {
                self.retain_message(message.clone(), Some(session));
                if action_failed {
                    self.present_error(message);
                } else {
                    self.notification_pending = true;
                    self.status = format!("{name}: session ended with an error.");
                }
            } else {
                self.status = format!("{name}: session closed.");
            }
        }
    }

    fn load_message_history(&mut self, state_root: &Path) {
        let store = ConfigStore::new(state_root);
        match store.load_messages() {
            Ok(messages) => self.messages = messages,
            Err(error) => self.status = format!("Could not read message history: {error}"),
        }
        self.message_store = Some(store);
    }

    fn watch_existing_sessions(&mut self) {
        for session in crate::session::scan_sessions(&crate::paths::runtime_sessions_dir()) {
            if session.health != crate::session::SessionHealth::Running {
                continue;
            }
            let name = self
                .profiles
                .iter()
                .find(|profile| profile.id == session.record.profile_id)
                .map_or_else(
                    || session.record.profile_id.to_string(),
                    |profile| profile.name.clone(),
                );
            self.launched_sessions.push(WatchedSession {
                id: session.record.session_id,
                name,
                initiated_here: false,
            });
        }
    }

    fn retain_message(&mut self, message: String, session_id: Option<SessionId>) {
        self.notification_pending = true;
        let mut entry = MessageEntry {
            created_at: now_secs(),
            message,
            session_id,
        };
        if let Some(store) = &self.message_store
            && let Err(error) = store.record_message(entry.clone())
        {
            let _ = write!(
                entry.message,
                "\n\nMessage history could not be saved: {error}. These details are available until the app closes."
            );
        }
        self.messages.push(entry);
        let excess = self.messages.len().saturating_sub(200);
        self.messages.drain(..excess);
    }

    fn queue_error(&mut self, message: String) {
        self.retain_message(message.clone(), None);
        self.present_error(message);
    }

    fn error_status(&mut self, message: String) -> String {
        self.queue_error(message);
        self.status.clone()
    }

    fn present_error(&mut self, message: String) {
        self.pending_errors.push_back(message);
        self.status = "Action failed — details pending.".to_string();
        self.present_pending_error();
    }

    fn present_pending_error(&mut self) {
        // Keep password entry, editing, and certificate confirmation intact.
        if self.error_dialog.is_none()
            && matches!(self.mode, Mode::Browsing)
            && let Some(message) = self.pending_errors.pop_front()
        {
            self.error_dialog = Some(TextView::new("Error", message));
            self.status = "Action failed — see dialog.".to_string();
        }
    }

    fn handle_error_dialog(&mut self, key: KeyEvent) {
        if self
            .error_dialog
            .as_mut()
            .is_some_and(|dialog| dialog.handle_key(key))
        {
            self.error_dialog = None;
            self.status = self.describe_current();
            self.present_pending_error();
        }
    }

    fn handle_status(&mut self, key: KeyEvent) {
        if let Mode::Status(details) = &mut self.mode
            && details.handle_key(key)
        {
            self.mode = Mode::Browsing;
            self.status = self.describe_current();
        }
    }

    fn begin_messages(&mut self) {
        self.notification_pending = false;
        let mut messages = self.messages.clone();
        // Session history also includes failures that finished while the TUI was closed.
        if let Some(store) = &self.message_store
            && let Ok(history) = store.load_history()
        {
            for entry in history.entries.iter().filter(|entry| !entry.succeeded()) {
                if entry.session_id.is_some()
                    && messages
                        .iter()
                        .any(|message| message.session_id == entry.session_id)
                {
                    continue;
                }
                let name = self
                    .profiles
                    .iter()
                    .find(|profile| profile.id == entry.profile_id)
                    .map_or_else(
                        || entry.profile_id.to_string(),
                        |profile| profile.name.clone(),
                    );
                messages.push(MessageEntry {
                    created_at: entry.finished_at,
                    message: session_message(entry, &name),
                    session_id: entry.session_id,
                });
            }
        }
        messages.sort_by_key(|message| std::cmp::Reverse(message.created_at));
        let mut text = String::from("Messages, newest first\n\n");
        for entry in messages {
            let _ = writeln!(
                text,
                "{}s ago\n{}\n",
                now_secs().saturating_sub(entry.created_at),
                entry.message
            );
        }
        if text == "Messages, newest first\n\n" {
            text.push_str("No messages yet.");
        }
        self.mode = Mode::Status(TextView::new("Messages", text));
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
            KeyCode::Char('p') => self.begin_password(CredentialSlot::Main),
            KeyCode::Char('g') => self.begin_password(CredentialSlot::Gateway),
            KeyCode::Char('T') => self.begin_certificate_mismatch(),
            KeyCode::Char('a') => self.begin_add(),
            KeyCode::Char('e') => self.begin_edit(),
            KeyCode::Char('c') => self.clone_current(),
            KeyCode::Char('d') => self.begin_delete(),
            KeyCode::Char('f' | '/') => self.begin_find(),
            KeyCode::Char('i') => self.begin_import(),
            KeyCode::Char('x') => self.begin_export(),
            KeyCode::Char('s') => self.begin_status(),
            KeyCode::Char('L') => self.toggle_diagnostics(),
            KeyCode::Char('h') => self.begin_history(),
            KeyCode::Char('m') => self.begin_messages(),
            KeyCode::Char('?') => self.begin_help(),
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
                if let Mode::Password { input, .. } = &mut self.mode {
                    input.pop();
                }
            }
            KeyCode::Char(character) => {
                if let Mode::Password { input, .. } = &mut self.mode {
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
                        self.run_deep_test(profile, true);
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
        match operations::save_profile(&self.store(), &self.config_root, profile) {
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
            Err(error) => self.set_form_error(&error),
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
        operations::strip_credentials(&mut copy);
        let (id, name) = (copy.id, copy.name.clone());
        match self.store().upsert(copy) {
            Ok(()) => {
                self.reload();
                self.select_id(id);
                self.status = format!("Cloned to {name}");
            }
            Err(error) => self.queue_error(format!("Could not clone: {error}")),
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
        let removed = self
            .profiles
            .iter()
            .find(|profile| profile.id == id)
            .cloned();
        let name = removed
            .as_ref()
            .map_or_else(|| id.to_string(), |profile| profile.name.clone());
        let result = removed.as_ref().map_or_else(
            || Ok(false),
            |profile| operations::delete_profile(&self.store(), &self.config_root, profile),
        );
        match result {
            Ok(_) => {
                self.reload();
                self.status = format!("Deleted {name}");
            }
            Err(error) => self.queue_error(format!("Could not delete: {error}")),
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
        match operations::import_profiles(&self.store(), Path::new(path)) {
            Ok(counts) => {
                self.reload();
                self.status = format!(
                    "Imported {}, updated {}, skipped {}, failed {}",
                    counts.added, counts.updated, counts.skipped, counts.failed
                );
                if counts.failed > 0 {
                    self.queue_error(self.status.clone());
                }
            }
            Err(error) => self.queue_error(format!("Import failed: {error}")),
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
            Err(error) => self.error_status(format!("Export failed: {error}")),
        };
    }

    fn begin_status(&mut self) {
        let now = now_secs();
        let history = ConfigStore::new(state_dir())
            .load_history()
            .unwrap_or_default();
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
            if let Some(entry) = history
                .entries
                .iter()
                .rev()
                .find(|entry| entry.profile_id == profile.id)
            {
                let _ = writeln!(
                    text,
                    "Last session: {}",
                    entry.summarize(&profile.name, now)
                );
            } else {
                let _ = writeln!(text, "Last session: never");
            }
        } else {
            let _ = writeln!(text, "No profile selected.");
        }
        let _ = writeln!(text, "\nActive sessions:");
        let mut any = false;
        for session in crate::session::scan_sessions(&crate::session::record::sessions_dir()) {
            any = true;
            let _ = writeln!(
                text,
                "  {} — {:?} (profile {})",
                session.record.session_id, session.health, session.record.profile_id
            );
        }
        if !any {
            let _ = writeln!(text, "  none active");
        }
        let diagnostic_path = crate::diagnostics::path();
        let _ = writeln!(
            text,
            "\nDiagnostics: {}",
            diagnostic_path
                .as_deref()
                .map_or_else(|| "disabled".into(), |path| path.display().to_string())
        );
        self.mode = Mode::Status(TextView::new("Status", text));
    }

    fn toggle_diagnostics(&mut self) {
        if let Some(path) = crate::diagnostics::path() {
            crate::diagnostics::disable();
            self.status = format!("Diagnostics disabled (log retained at {})", path.display());
            return;
        }
        let path = state_dir().join("diagnostics.log");
        self.status = match crate::diagnostics::enable(path.clone()) {
            Ok(()) => format!("Diagnostics enabled: {}", path.display()),
            Err(error) => self.error_status(format!("Could not enable diagnostics: {error}")),
        };
    }

    fn begin_history(&mut self) {
        let now = now_secs();
        let history = ConfigStore::new(state_dir())
            .load_history()
            .unwrap_or_default();
        let mut text = String::from("Recent connections:\n");
        let mut shown = 0_usize;
        for entry in history.entries.iter().rev() {
            let name = self
                .profiles
                .iter()
                .find(|profile| profile.id == entry.profile_id)
                .map_or_else(
                    || format!("(removed {})", entry.profile_id),
                    |profile| profile.name.clone(),
                );
            let _ = writeln!(text, "  {}", entry.summarize(&name, now));
            shown += 1;
        }
        if shown == 0 {
            text.push_str("  none yet\n");
        }
        self.mode = Mode::Status(TextView::new("Connections", text));
    }

    fn begin_help(&mut self) {
        let mut text = String::from("Keys:\n\n");
        for (key, description) in [
            ("Up/Down, j/k", "move selection"),
            ("Enter", "connect"),
            ("a", "add a profile"),
            ("e", "edit selected profile"),
            ("c", "clone"),
            ("d", "delete (type yes)"),
            ("f, /", "find / filter"),
            ("i", "import (Remmina/.rdp/dir)"),
            ("x", "export to .rdp"),
            ("s", "status"),
            ("L", "toggle diagnostic logging"),
            ("h", "connection history"),
            ("m", "messages and error details"),
            ("D", "deep-test credentials"),
            ("p", "set / clear main password"),
            ("g", "set / clear gateway password"),
            ("T", "review changed certificate"),
            ("t", "reachability test"),
            ("?", "this help"),
            ("q, Esc", "quit"),
        ] {
            let _ = writeln!(text, "  {key:<14} {description}");
        }
        text.push_str("\nEdit form: Up/Down move, Enter/Space edit, A accept, Esc cancel");
        self.mode = Mode::Status(TextView::new("Help", text));
    }

    fn connect(&mut self) {
        let Some(profile) = self.current().cloned() else {
            return;
        };
        match connect_profile(&profile, &self.executable) {
            Ok(session) => {
                self.launched_sessions.push(WatchedSession {
                    id: session,
                    name: profile.name.clone(),
                    initiated_here: true,
                });
                self.status = format!("Starting session {session} for {}…", profile.name);
            }
            Err(error) => self.queue_error(format!("{}: Connect failed: {error}", profile.name)),
        }
    }

    fn test(&mut self) {
        let Some(profile) = self.current().cloned() else {
            return;
        };
        if self.background_busy {
            self.status = "A connection check is already running.".into();
            return;
        }
        self.background_busy = true;
        self.status = format!("Testing {}…", profile.name);
        let sender = self.background_tx.clone();
        std::thread::spawn(move || {
            let status = match test_profile(&profile, TEST_TIMEOUT) {
                Ok(()) => BackgroundResult::Status(format!("{} is reachable", profile.name)),
                Err(error) => BackgroundResult::Error(format!("{}: {error}", profile.name)),
            };
            let _ = sender.send(status);
        });
    }

    fn deep_test(&mut self) {
        let Some(profile) = self.current().cloned() else {
            return;
        };
        if deep_test_requires_acknowledgement(&profile, &state_dir()) {
            self.mode = Mode::Prompt {
                label: format!("{} Deep-test {}? type yes", DEEP_TEST_WARNING, profile.name),
                input: String::new(),
                action: PromptAction::ConfirmDeepTest(profile.id),
            };
        } else {
            self.run_deep_test(profile, false);
        }
    }

    fn run_deep_test(&mut self, profile: Profile, acknowledged: bool) {
        if self.background_busy {
            self.status = "A connection check is already running.".into();
            return;
        }
        self.background_busy = true;
        self.status = format!("Deep-testing {}…", profile.name);
        let sender = self.background_tx.clone();
        let config_root = self.config_root.clone();
        let state_root = state_dir();
        std::thread::spawn(move || {
            let store = SystemCredentialStore::new(config_root);
            let status = match crate::session::deep_test_profile(
                &profile,
                &store,
                &state_root,
                acknowledged,
            ) {
                Ok(outcome) => {
                    let message = format!("{}: {}", profile.name, deep_test_message(outcome));
                    if matches!(
                        outcome,
                        crate::session::DeepTest::Authenticated
                            | crate::session::DeepTest::RateLimited
                            | crate::session::DeepTest::NeedsAcknowledgement
                    ) {
                        BackgroundResult::Status(message)
                    } else {
                        BackgroundResult::Error(message)
                    }
                }
                Err(error) => {
                    BackgroundResult::Error(format!("{}: deep-test failed: {error}", profile.name))
                }
            };
            let _ = sender.send(status);
        });
    }

    fn begin_password(&mut self, slot: CredentialSlot) {
        let Some(profile) = self.current() else {
            return;
        };
        if slot == CredentialSlot::Gateway && !matches!(profile.route, Route::RdGateway { .. }) {
            self.status = "Set this profile's route to an RD Gateway first.".into();
            return;
        }
        let name = profile.name.clone();
        self.mode = Mode::Password {
            slot,
            input: Zeroizing::new(String::new()),
        };
        let label = if slot == CredentialSlot::Main {
            "Password"
        } else {
            "Gateway password"
        };
        self.status =
            format!("{label} for {name} — Enter to save, empty Enter to clear, Esc to cancel");
    }

    fn commit_password(&mut self) {
        let (slot, password) = match &self.mode {
            Mode::Password { slot, input } => (*slot, input.to_string()),
            _ => return,
        };
        self.mode = Mode::Browsing;
        let Some(index) = self.current_index() else {
            return;
        };
        let name = self.profiles[index].name.clone();
        if password.is_empty() {
            // Empty = clear any saved password (parity with `credential clear`).
            self.status = match self.clear_password(index, slot) {
                Ok(true) => format!("Cleared the password for {name}"),
                Ok(false) => format!("{name} had no saved password"),
                Err(error) => self.error_status(format!("Could not clear password: {error}")),
            };
            return;
        }
        self.status = match self.save_password(index, slot, &password) {
            Ok(()) => format!("Saved a password for {name}"),
            Err(error) => self.error_status(format!("Could not save password: {error}")),
        };
    }

    fn clear_password(&mut self, index: usize, slot: CredentialSlot) -> Result<bool, String> {
        let (profile, cleared) = operations::clear_password(
            &self.store(),
            self.config_root.as_path(),
            self.profiles[index].clone(),
            slot,
        )?;
        self.profiles[index] = profile;
        Ok(cleared)
    }

    fn save_password(
        &mut self,
        index: usize,
        slot: CredentialSlot,
        password: &str,
    ) -> Result<(), String> {
        let profile = operations::replace_password(
            &self.store(),
            self.config_root.as_path(),
            self.profiles[index].clone(),
            slot,
            password,
        )?;
        // Keep the in-memory profile consistent so the next connect uses it.
        self.profiles[index] = profile;
        Ok(())
    }

    fn begin_certificate_mismatch(&mut self) {
        let Some(profile) = self.current() else {
            return;
        };
        match crate::freerdp::certificate::read_mismatch(&state_dir(), profile.id) {
            Ok(Some(mismatch)) => self.mode = Mode::CertificateMismatch(mismatch),
            Ok(None) => self.status = "No changed certificate is awaiting confirmation.".into(),
            Err(error) => self.queue_error(format!("Could not read certificate status: {error}")),
        }
    }

    fn handle_certificate_mismatch(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('T') => {
                let Mode::CertificateMismatch(mismatch) = &self.mode else {
                    return;
                };
                let fingerprint = mismatch.presented_sha256.clone();
                let profile_id = mismatch.profile_id;
                let Some(profile) = self
                    .profiles
                    .iter()
                    .find(|profile| profile.id == profile_id)
                    .cloned()
                else {
                    self.mode = Mode::Browsing;
                    self.status = "The profile no longer exists.".into();
                    return;
                };
                self.mode = Mode::Browsing;
                self.status = match operations::trust_certificate_mismatch(
                    &self.store(),
                    self.config_root.as_path(),
                    &state_dir(),
                    profile,
                    &fingerprint,
                ) {
                    Ok(_) => "Certificate approved; reconnect to pin the replacement.".into(),
                    Err(error) => {
                        self.error_status(format!("Could not approve certificate: {error}"))
                    }
                };
            }
            KeyCode::Char('q' | 'Q') | KeyCode::Esc => {
                self.mode = Mode::Browsing;
                self.status = "Certificate change left untrusted.".into();
            }
            _ => {}
        }
    }

    fn draw(&mut self, frame: &mut ratatui::Frame) {
        view::draw(self, frame);
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
fn profile_matches(profile: &Profile, query: &str) -> bool {
    profile.name.to_lowercase().contains(query)
        || profile.endpoint.to_string().to_lowercase().contains(query)
        || profile.identity.username.to_lowercase().contains(query)
        || profile.identity.domain.to_lowercase().contains(query)
}

#[cfg(test)]
mod tests {
    use super::{
        App, BackgroundResult, CredentialSlot, EditForm, FIELDS, Field, Mode, PromptAction,
    };
    use crate::config::ConfigStore;
    use crate::model::{
        CertificatePolicy, DeviceConfig, DisplayConfig, Endpoint, GraphicsMode, IdentityConfig,
        Profile, ProfileId, Renderer, Route, SecurityConfig,
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
    fn normal_launch_reports_its_authentication_failure_once_and_ignores_old_attempts() {
        use crate::model::{ConnectionFailure, HistoryEntry, SessionId};
        let state = TempDir::new().unwrap();
        let store = ConfigStore::new(state.path());
        let mut app = app_with(&["Anima"]);
        let profile = app.current().unwrap().id;
        let session = SessionId::generate();
        let mut entry = HistoryEntry {
            profile_id: profile,
            session_id: Some(SessionId::generate()),
            finished_at: 1000,
            duration_ms: 100,
            exit_code: Some(134),
            failure: Some(ConnectionFailure::Authentication),
            renderer: Renderer::X11,
            established: false,
            ended_by_user: false,
        };
        store.record_history(entry.clone(), 200).unwrap();
        app.launched_sessions.push(super::WatchedSession {
            id: session,
            name: "Anima".to_string(),
            initiated_here: true,
        });
        app.status = "Starting Anima".to_string();
        app.receive_session_results(state.path());
        assert_eq!(app.status, "Starting Anima");
        assert_eq!(app.launched_sessions.len(), 1);

        entry.session_id = Some(session);
        store.record_history(entry, 200).unwrap();
        app.receive_session_results(state.path());
        let dialog = app
            .error_dialog
            .as_ref()
            .expect("failed launches open an error dialog");
        assert!(dialog.message.contains("Anima"));
        assert!(dialog.message.contains("host rejected the credentials"));
        assert!(dialog.message.contains("press p"));
        assert!(!app.status.contains("host rejected the credentials"));
        assert!(app.launched_sessions.is_empty());
        let screen = rendered(&mut app, 80, 24);
        assert!(screen.contains("authentication failed"));
        assert!(screen.contains("press p"));

        app.status = "Password updated".to_string();
        app.receive_session_results(state.path());
        assert_eq!(app.status, "Password updated");
    }

    #[test]
    fn error_dialog_scrolls_to_the_end_in_small_terminals_and_clamps_after_resize() {
        let mut app = app_with(&["Anima"]);
        let message = format!(
            "START_MARKER\n{}\nTAIL_SENTINEL",
            "Long diagnostic detail to wrap across several lines.\n".repeat(20)
        );
        app.queue_error(message);
        for (width, height) in [(80, 24), (40, 12), (24, 8), (80, 24)] {
            rendered(&mut app, width, height);
            app.handle_error_dialog(press(KeyCode::End));
            let screen = rendered(&mut app, width, height);
            assert!(
                screen.contains("TAIL_SENTINEL"),
                "missing message end at {width}x{height}"
            );
            assert!(
                app.error_dialog.is_some(),
                "scrolling must not dismiss the dialog"
            );
            app.handle_error_dialog(press(KeyCode::Down));
            assert!(rendered(&mut app, width, height).contains("TAIL_SENTINEL"));
            app.handle_error_dialog(press(KeyCode::Home));
            assert!(rendered(&mut app, width, height).contains("START_MARKER"));
        }
        app.handle_error_dialog(press(KeyCode::Enter));
        assert!(app.error_dialog.is_none());
        assert!(matches!(app.mode, Mode::Browsing));
    }

    #[test]
    fn errors_wait_for_password_entry_and_queue_without_replacing_the_current_dialog() {
        let mut app = app_with(&["Anima"]);
        app.begin_password(CredentialSlot::Main);
        app.handle_password(press(KeyCode::Char('x')));
        app.queue_error("First failure".to_string());
        app.queue_error("Second failure".to_string());
        assert!(app.error_dialog.is_none());
        assert!(matches!(&app.mode, Mode::Password { input, .. } if input.as_str() == "x"));
        app.handle_password(press(KeyCode::Esc));
        app.present_pending_error();
        assert_eq!(app.error_dialog.as_ref().unwrap().message, "First failure");
        app.queue_error("Third failure".to_string());
        assert_eq!(app.error_dialog.as_ref().unwrap().message, "First failure");
        app.handle_error_dialog(press(KeyCode::Esc));
        assert_eq!(app.error_dialog.as_ref().unwrap().message, "Second failure");
        app.handle_error_dialog(press(KeyCode::Enter));
        assert_eq!(app.error_dialog.as_ref().unwrap().message, "Third failure");
        app.handle_error_dialog(press(KeyCode::Esc));
        assert!(app.error_dialog.is_none());
    }

    #[test]
    fn failed_connection_checks_open_the_dialog_while_success_stays_in_the_footer() {
        let mut app = app_with(&["Anima"]);
        app.background_tx
            .send(BackgroundResult::Status("Anima is reachable".to_string()))
            .unwrap();
        app.receive_background_result();
        assert_eq!(app.status, "Anima is reachable");
        assert!(app.error_dialog.is_none());
        app.background_tx
            .send(BackgroundResult::Error(
                "Anima: the host rejected the credentials".to_string(),
            ))
            .unwrap();
        app.receive_background_result();
        assert!(
            app.error_dialog
                .as_ref()
                .unwrap()
                .message
                .contains("host rejected the credentials")
        );
    }

    #[test]
    fn established_session_errors_notify_without_stealing_input_and_survive_restart() {
        use crate::model::{ConnectionFailure, HistoryEntry, SessionId};
        let state = TempDir::new().unwrap();
        let mut app = app_with(&["Anima"]);
        app.load_message_history(state.path());
        let session = SessionId::generate();
        let entry = HistoryEntry {
            profile_id: app.current().unwrap().id,
            session_id: Some(session),
            finished_at: 1000,
            duration_ms: 100,
            exit_code: Some(134),
            failure: Some(ConnectionFailure::ProcessFailure),
            renderer: Renderer::X11,
            established: true,
            ended_by_user: false,
        };
        ConfigStore::new(state.path())
            .record_history(entry, 200)
            .unwrap();
        app.launched_sessions.push(super::WatchedSession {
            id: session,
            name: "Anima".to_string(),
            initiated_here: true,
        });
        app.begin_password(CredentialSlot::Main);
        app.handle_password(press(KeyCode::Char('x')));
        app.receive_session_results(state.path());
        assert!(app.error_dialog.is_none());
        assert!(app.pending_errors.is_empty());
        assert!(app.notification_pending);
        assert!(matches!(&app.mode, Mode::Password { input, .. } if input.as_str() == "x"));
        app.handle_password(press(KeyCode::Esc));
        app.present_pending_error();
        assert!(
            app.error_dialog.is_none(),
            "background errors must not become modal after editing"
        );
        let before = app.messages.clone();
        let mut restarted = app_with(&["Anima"]);
        restarted.load_message_history(state.path());
        assert_eq!(restarted.messages, before);
        restarted.handle_browsing(press(KeyCode::Char('m')));
        assert!(
            matches!(&restarted.mode, Mode::Status(details) if details.message.contains("Anima") && details.message.contains("enable diagnostics"))
        );
    }

    #[test]
    fn dismissed_errors_reopen_in_scrollable_message_history_with_the_full_tail() {
        let state = TempDir::new().unwrap();
        let mut app = app_with(&["Anima"]);
        app.load_message_history(state.path());
        let message = format!(
            "Failure\n{}\nTAIL_SENTINEL",
            "Diagnostic details to read later.\n".repeat(40)
        );
        app.queue_error(message.clone());
        app.handle_error_dialog(press(KeyCode::Esc));
        assert!(app.error_dialog.is_none());
        assert_eq!(app.messages[0].message, message);
        app.handle_browsing(press(KeyCode::Char('m')));
        assert!(!app.notification_pending);
        for (width, height) in [(80, 24), (24, 8), (40, 12)] {
            rendered(&mut app, width, height);
            app.handle_status(press(KeyCode::End));
            assert!(rendered(&mut app, width, height).contains("TAIL_SENTINEL"));
            assert!(matches!(app.mode, Mode::Status(_)));
        }
        app.handle_status(press(KeyCode::Esc));
        assert!(matches!(app.mode, Mode::Browsing));
    }

    #[test]
    fn expected_user_disconnects_are_short_status_messages() {
        use crate::model::{HistoryEntry, SessionId};
        let state = TempDir::new().unwrap();
        let mut app = app_with(&["Anima"]);
        let session = SessionId::generate();
        let entry = HistoryEntry {
            profile_id: app.current().unwrap().id,
            session_id: Some(session),
            finished_at: 1000,
            duration_ms: 100,
            exit_code: Some(131),
            failure: None,
            renderer: Renderer::X11,
            established: true,
            ended_by_user: true,
        };
        ConfigStore::new(state.path())
            .record_history(entry, 200)
            .unwrap();
        app.launched_sessions.push(super::WatchedSession {
            id: session,
            name: "Anima".to_string(),
            initiated_here: true,
        });
        app.receive_session_results(state.path());
        assert!(app.error_dialog.is_none());
        assert!(!app.notification_pending);
        assert!(app.messages.is_empty());
        assert_eq!(app.status, "Anima: session closed.");
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
        app.begin_password(CredentialSlot::Main);
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
        app.begin_password(CredentialSlot::Main);
        assert!(matches!(app.mode, Mode::Password { .. }));
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

        app.begin_password(CredentialSlot::Main);
        for character in "hunter2".chars() {
            app.handle_password(press(KeyCode::Char(character)));
        }
        app.handle_password(press(KeyCode::Enter));
        assert!(app.profiles[0].credential.is_some());

        // Re-open and submit nothing: the saved password is cleared.
        app.begin_password(CredentialSlot::Main);
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
        // Cycle the graphics pipeline (Auto -> Rfx).
        go_to_field(&mut app, Field::Graphics);
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
        assert_eq!(saved.display.graphics, GraphicsMode::Rfx);
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
        app.begin_password(CredentialSlot::Main);
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
    fn help_overlay_lists_the_keys() {
        let mut app = app_with(&["Anima"]);
        app.handle_browsing(press(KeyCode::Char('?')));
        assert!(matches!(app.mode, Mode::Status(_)));
        let screen = rendered(&mut app, 80, 24);
        assert!(screen.contains("add a profile"));
        assert!(screen.contains("connection history"));
    }

    #[test]
    fn history_overlay_opens_with_a_header() {
        let mut app = app_with(&["Anima"]);
        app.handle_browsing(press(KeyCode::Char('h')));
        assert!(matches!(app.mode, Mode::Status(_)));
        assert!(rendered(&mut app, 80, 24).contains("Recent connections"));
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
