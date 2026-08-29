//! Shared one-shot session execution for future CLI and TUI callers.

pub mod launcher;
pub mod record;
pub mod supervisor;

use crate::credentials::askpass::AskpassLease;
use crate::credentials::{CredentialError, CredentialStore, acquire};
use crate::freerdp::capabilities::AuthOnlySupport;
use crate::freerdp::deep_test::{AuthOutcome, authenticate};
use crate::freerdp::discover::discover;
use crate::freerdp::process::launch;
use crate::model::{
    ConnectionFailure, ConnectionPlan, PreparedConnection, Profile, Renderer, ResolvedCredentials,
    RouteHandle, SessionId, SessionResult,
};
use crate::planner::plan;
use crate::preflight::{prepare_for_session, verify_prepared};
use crate::runtime::process::LaunchMode;
use crate::runtime::registry::still_matches;
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
    Credential(CredentialError),
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
            Self::Credential(error) => write!(formatter, "credential acquisition failed: {error}"),
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
        let mut child = launch(prepared, session, askpass, LaunchMode::OneShot, None)?;
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

/// The result of an auth-only deep-test.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum DeepTest {
    /// The stored credentials authenticated against the host.
    Authenticated,
    /// The host rejected the credentials.
    AuthFailed,
    /// The host could not be reached to attempt authentication.
    Unreachable,
    /// This `FreeRDP` build's auth-only mode is not version-validated.
    NotSupported,
    /// Skipped: a deep-test ran too recently for this profile (a courtesy, not
    /// a lockout guarantee).
    RateLimited,
    /// The first deep-test of this profile: the caller must show
    /// [`DEEP_TEST_WARNING`] and re-run with `acknowledged = true` to proceed.
    NeedsAcknowledgement,
}

/// Shown once, before the first deep-test of a profile (DEC-deep-test-framing):
/// a deep-test performs a real authentication, and repeated failures can trip an
/// account-lockout policy rdp-tui cannot observe.
pub const DEEP_TEST_WARNING: &str = "A deep-test performs a real authentication against the host. \
Repeated failures can trip the account's lockout policy, which rdp-tui cannot observe or undo. \
This confirmation is asked once per profile.";

/// Minimum spacing between deep-tests of one profile — a courtesy that reduces
/// the chance of tripping a target account-lockout policy rdp-tui cannot observe.
const DEEP_TEST_INTERVAL: Duration = Duration::from_secs(30);

/// Verify a profile's stored credentials with `FreeRDP`'s auth-only mode. Never
/// automatic and always explicit (DEC-deep-test); gated on a version-validated
/// capability and rate-limited through a persisted per-profile timestamp.
///
/// The first deep-test of a profile returns [`DeepTest::NeedsAcknowledgement`]
/// unless `acknowledged` is set, so a caller can show [`DEEP_TEST_WARNING`] once.
/// The per-profile stamp file records that a deep-test has run, so the warning is
/// never shown again.
///
/// # Errors
///
/// Returns [`ConnectError`] when no X11 client is available, credentials cannot
/// be resolved, or the auth-only attempt cannot be run.
pub fn deep_test_profile(
    profile: &Profile,
    store: &impl CredentialStore,
    state_dir: &Path,
    acknowledged: bool,
) -> Result<DeepTest, ConnectError> {
    let stamp = state_dir
        .join("deep_test")
        .join(format!("{}.json", profile.id));
    // The one-time warning gate runs before any discovery or network work, so an
    // unacknowledged first deep-test spawns nothing.
    if needs_acknowledgement(&stamp, acknowledged) {
        return Ok(DeepTest::NeedsAcknowledgement);
    }
    // Auth-only runs headless, so always use the X11 client.
    let discovered = discover(Renderer::X11).map_err(ConnectError::Discover)?;
    if discovered.capabilities.auth_only != AuthOnlySupport::Validated {
        return Ok(DeepTest::NotSupported);
    }
    if recently_deep_tested(&stamp) {
        return Ok(DeepTest::RateLimited);
    }
    let references = ResolvedCredentials {
        main: profile.credential,
        gateway: None,
    };
    let lease = acquire(store, references).map_err(ConnectError::Credential)?;
    let outcome = authenticate(
        &discovered.client.executable,
        &profile.endpoint,
        &profile.identity,
        lease.main.as_ref(),
    )
    .map_err(ConnectError::Io)?;
    record_deep_test(&stamp);
    Ok(match outcome {
        AuthOutcome::Authenticated => DeepTest::Authenticated,
        AuthOutcome::LogonFailure => DeepTest::AuthFailed,
        AuthOutcome::Unreachable => DeepTest::Unreachable,
    })
}

/// The one-time-warning gate: the first deep-test of a profile (no stamp yet)
/// needs acknowledgement; once a stamp exists the profile has been deep-tested
/// before, so the warning is never repeated.
fn needs_acknowledgement(stamp: &Path, acknowledged: bool) -> bool {
    !acknowledged && !stamp.exists()
}

fn recently_deep_tested(stamp: &Path) -> bool {
    std::fs::metadata(stamp)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|elapsed| elapsed < DEEP_TEST_INTERVAL)
}

fn record_deep_test(stamp: &Path) {
    if let Some(parent) = stamp.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // The file's mtime is the rate-limit clock.
    let _ = std::fs::write(stamp, b"{}\n");
}

/// Health of a detached session as seen from its record.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum SessionHealth {
    /// The supervisor process is still alive and owns the session.
    Running,
    /// The supervisor is gone but a child (`FreeRDP` or tunnel) is still alive —
    /// inconsistent ownership that `doctor` reports for an explicit decision.
    Inconsistent,
    /// Nothing the record referenced is still running; the record was removed.
    Stale,
}

/// One session record paired with its current health.
#[derive(Debug, Clone)]
pub struct SessionStatus {
    pub record: record::SessionRecord,
    pub health: SessionHealth,
}

/// Reconcile the session records in `dir` against `/proc`, removing records
/// whose processes are all gone (`session_supervisor` `failure_recovery`). This
/// only reports and prunes stale records; it never signals or terminates a
/// process, and every liveness test is a full compound-identity match (INV-11).
#[must_use]
pub fn scan_sessions(dir: &Path) -> Vec<SessionStatus> {
    let mut statuses = Vec::new();
    for record in record::list(dir).unwrap_or_default() {
        let health = if still_matches(record.supervisor) {
            SessionHealth::Running
        } else if record.freerdp.is_some_and(still_matches)
            || record.tunnel.is_some_and(still_matches)
        {
            SessionHealth::Inconsistent
        } else {
            let _ = record::remove(dir, record.session_id);
            SessionHealth::Stale
        };
        statuses.push(SessionStatus { record, health });
    }
    statuses
}

#[cfg(test)]
mod tests {
    use super::{
        DEEP_TEST_INTERVAL, needs_acknowledgement, recently_deep_tested, record_deep_test,
    };
    use std::time::Duration;
    use tempfile::TempDir;

    #[test]
    fn first_deep_test_of_a_profile_needs_acknowledgement() {
        let dir = TempDir::new().unwrap();
        let stamp = dir.path().join("deep_test").join("profile.json");
        // No stamp yet: an unacknowledged first deep-test must be gated.
        assert!(needs_acknowledgement(&stamp, false));
        // Explicit acknowledgement lets the first one through.
        assert!(!needs_acknowledgement(&stamp, true));
    }

    #[test]
    fn once_a_deep_test_has_run_no_further_acknowledgement_is_asked() {
        let dir = TempDir::new().unwrap();
        let stamp = dir.path().join("deep_test").join("profile.json");
        record_deep_test(&stamp);
        assert!(stamp.exists());
        // A recorded stamp means the profile has been deep-tested before.
        assert!(!needs_acknowledgement(&stamp, false));
    }

    #[test]
    fn the_rate_limit_is_persisted_so_a_separate_process_would_observe_it() {
        let dir = TempDir::new().unwrap();
        let stamp = dir.path().join("deep_test").join("profile.json");
        record_deep_test(&stamp);
        // A fresh stamp file (what any process reading the same path would see)
        // blocks a second run inside the courtesy interval.
        assert!(recently_deep_tested(&stamp));

        // Age the stamp past the interval; the limit no longer applies.
        let past = std::time::SystemTime::now() - (DEEP_TEST_INTERVAL + Duration::from_secs(5));
        let file = std::fs::File::open(&stamp).unwrap();
        file.set_modified(past).unwrap();
        assert!(!recently_deep_tested(&stamp));
    }
}
