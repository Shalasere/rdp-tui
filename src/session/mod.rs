//! Shared one-shot session execution for future CLI and TUI callers.

pub mod launcher;
pub mod record;
pub mod supervisor;

use crate::credentials::askpass::AskpassLease;
use crate::credentials::{CredentialError, CredentialStore, acquire};
use crate::freerdp::discover::discover;
use crate::freerdp::process::launch;
use crate::model::{
    ConnectionFailure, ConnectionPlan, PreparedConnection, Profile, Renderer, RouteHandle,
    SessionId, SessionResult,
};
use crate::planner::plan;
use crate::preflight::{prepare_for_session, verify_prepared};
use crate::runtime::process::LaunchMode;
use crate::ssh::tunnel::terminate;
use std::path::Path;
use std::time::{Duration, Instant};

/// Failure while acquiring credentials or running a prepared session.
#[derive(Debug)]
pub enum SessionError {
    Credential(CredentialError),
    Io(std::io::Error),
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Credential(error) => write!(formatter, "credential acquisition failed: {error}"),
            Self::Io(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for SessionError {}

impl From<std::io::Error> for SessionError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<CredentialError> for SessionError {
    fn from(error: CredentialError) -> Self {
        Self::Credential(error)
    }
}

/// Failure while planning or starting a session from a profile.
#[derive(Debug)]
pub enum ConnectError {
    Discover(String),
    Plan(ConnectionFailure),
    Preflight(ConnectionFailure),
    Io(std::io::Error),
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Discover(message) => write!(formatter, "no usable FreeRDP client: {message}"),
            Self::Plan(failure) => write!(formatter, "cannot plan connection: {failure:?}"),
            Self::Preflight(failure) => {
                write!(formatter, "connection is not reachable: {failure:?}")
            }
            Self::Io(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for ConnectError {}

/// Launch a prepared connection and return its observable process result.
///
/// # Errors
///
/// Returns an I/O error when the process cannot be started or waited for.
pub fn run(mut prepared: PreparedConnection, session: SessionId) -> std::io::Result<SessionResult> {
    run_with_askpass(&mut prepared, session, None)
}

/// Resolve credentials, prepare their sealed ASKPASS lease, and run a session.
///
/// # Errors
///
/// Returns [`SessionError::Credential`] when a persisted reference cannot be
/// resolved, or [`SessionError::Io`] when the lease or child process fails.
pub fn run_with_credentials(
    mut prepared: PreparedConnection,
    session: SessionId,
    store: &impl CredentialStore,
    helper: &Path,
) -> Result<SessionResult, SessionError> {
    let lease = acquire(store, prepared.plan.credentials)?;
    let askpass = AskpassLease::prepare(&lease, helper.to_path_buf())?;
    run_with_askpass(&mut prepared, session, Some(&askpass)).map_err(SessionError::Io)
}

fn run_with_askpass(
    prepared: &mut PreparedConnection,
    session: SessionId,
    askpass: Option<&AskpassLease>,
) -> std::io::Result<SessionResult> {
    let started = Instant::now();
    let result = (|| {
        // A foreground session dies with its launcher; a detached connect
        // owner is the session supervisor's responsibility, not this runner.
        let mut child = launch(prepared, session, askpass, LaunchMode::OneShot)?;
        let status = child.wait()?;
        Ok(SessionResult {
            duration: started.elapsed(),
            exit_code: status.code(),
            failure: (!status.success()).then_some(ConnectionFailure::ProcessFailure),
            renderer: prepared.plan.client.renderer,
            freerdp_version: prepared.plan.client.version.clone(),
        })
    })();
    if let Some(RouteHandle::SshTunnel(tunnel)) = &mut prepared.route_handle {
        terminate(tunnel)?;
    }
    result
}

/// Plan a profile, mint a session, and launch a detached supervisor for it.
///
/// Shared by the CLI and TUI so both start connections the same way (INV-8).
///
/// # Errors
///
/// Returns [`ConnectError`] when the profile cannot be planned or the detached
/// supervisor cannot be spawned.
pub fn connect_profile(profile: &Profile, executable: &Path) -> Result<SessionId, ConnectError> {
    let mut plan = plan_profile(profile)?;
    fill_sdl_fullscreen_size(&mut plan);
    let session = SessionId::generate();
    launcher::spawn_supervisor(&plan, profile.id, session, executable).map_err(ConnectError::Io)?;
    Ok(session)
}

/// SDL fullscreen cannot use `FreeRDP`'s `/f` (it reads a 64x64 monitor and fails
/// pre-connect), so give it the focused monitor's resolution as an explicit
/// `/size`, matching the Python client. Falls back to 1920x1080 if the monitor
/// cannot be detected. Runs in the graphical launcher, before the plan is sent
/// to the detached supervisor.
fn fill_sdl_fullscreen_size(plan: &mut ConnectionPlan) {
    if plan.display.fullscreen
        && matches!(plan.client.renderer, Renderer::WaylandSdl)
        && plan.display.resolution.is_none()
    {
        plan.display.resolution = Some(detect_primary_resolution().unwrap_or((1920, 1080)));
    }
}

/// Return the focused Hyprland monitor's physical resolution via `hyprctl`.
fn detect_primary_resolution() -> Option<(u16, u16)> {
    let output = std::process::Command::new("hyprctl")
        .args(["monitors", "-j"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let monitors: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    let list = monitors.as_array()?;
    let monitor = list
        .iter()
        .find(|entry| entry.get("focused").and_then(serde_json::Value::as_bool) == Some(true))
        .or_else(|| list.first())?;
    let width = u16::try_from(monitor.get("width")?.as_u64()?).ok()?;
    let height = u16::try_from(monitor.get("height")?.as_u64()?).ok()?;
    if (200..=16_384).contains(&width) && (200..=16_384).contains(&height) {
        Some((width, height))
    } else {
        None
    }
}

/// One-shot reachability check: acquire the route, verify the same prepared
/// connection, then release any retained tunnel again. Shared by CLI and TUI.
///
/// # Errors
///
/// Returns [`ConnectError`] when the profile cannot be planned, the route cannot
/// be acquired, or the endpoint is unreachable.
pub fn test_profile(profile: &Profile, timeout: Duration) -> Result<(), ConnectError> {
    let plan = plan_profile(profile)?;
    let session = SessionId::generate();
    let mut prepared = prepare_for_session(&plan, session).map_err(ConnectError::Preflight)?;
    let reachable = verify_prepared(&prepared, timeout);
    if let Some(RouteHandle::SshTunnel(handle)) = &mut prepared.route_handle {
        let _ = terminate(handle);
    }
    reachable.map_err(ConnectError::Preflight)
}

fn plan_profile(profile: &Profile) -> Result<ConnectionPlan, ConnectError> {
    let discovered = discover(profile.display.renderer).map_err(ConnectError::Discover)?;
    plan(profile, &discovered.capabilities, discovered.client).map_err(ConnectError::Plan)
}
