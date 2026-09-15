//! Detached connect-session owner: the logic that runs inside the supervisor
//! process once the launcher has handed it a plan. See the `session_supervisor`
//! contract in `docs/architecture/04-amendments.yaml`.

use crate::config::ConfigStore;
use crate::credentials::askpass::AskpassLease;
use crate::credentials::{CredentialError, CredentialStore, acquire};
use crate::freerdp::process::launch;
use crate::model::{
    ConnectionFailure, ConnectionPlan, FreeRdpClient, HistoryEntry, PreparedConnection, ProfileId,
    Renderer, RouteHandle, SessionId, SessionResult,
};
use crate::preflight::{PreflightError, prepare_for_session, verify_prepared};
use crate::runtime::process::{LaunchMode, OwnedChild};
use crate::runtime::registry::{ChildKind, ProcessIdentity, observe};
use crate::session::record::{self, SessionRecord, SessionRecordState};
use crate::ssh::tunnel::terminate;
use std::io::Read as _;
use std::path::Path;
use std::time::{Duration, Instant};

/// How long a Wayland/SDL client has to bring up a window before an early
/// non-zero exit is treated as "failed before mapping" and retried on X11.
/// Mirrors the Python map-wait window (`fullscreen_wayland_sdl_window`, 3s).
const MAP_GRACE: Duration = Duration::from_secs(3);

/// Failure while supervising a detached connect session.
#[derive(Debug)]
pub enum SuperviseError {
    Credential(CredentialError),
    Preflight(PreflightError),
    Io(std::io::Error),
}

impl std::fmt::Display for SuperviseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Credential(error) => write!(formatter, "credential acquisition failed: {error}"),
            Self::Preflight(error) => write!(formatter, "preflight failed: {error}"),
            Self::Io(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for SuperviseError {}

/// Own one detached connect session for its whole lifetime: resolve secrets,
/// acquire the route, preflight the *same* prepared connection, launch
/// `FreeRDP`, record the live session, and reap the route when `FreeRDP` exits.
///
/// The session record is always removed on the way out; a crash intentionally
/// leaves it behind for `doctor` to reconcile.
///
/// # Errors
///
/// Returns [`SuperviseError`] when credentials, preflight, the child process,
/// or the runtime record cannot be handled.
#[allow(clippy::too_many_arguments)]
pub fn supervise(
    plan: &ConnectionPlan,
    profile_id: ProfileId,
    session: SessionId,
    store: &impl CredentialStore,
    helper: &Path,
    config_root: &Path,
    records_dir: &Path,
    state_dir: &Path,
    preflight_timeout: Duration,
) -> Result<SessionResult, SuperviseError> {
    let supervisor =
        observe(std::process::id(), ChildKind::Supervisor, session).map_err(SuperviseError::Io)?;
    let outcome = run_supervised(
        plan,
        profile_id,
        session,
        store,
        helper,
        config_root,
        records_dir,
        state_dir,
        preflight_timeout,
        supervisor,
    );
    let _ = record::remove(records_dir, session);
    if let Ok(result) = &outcome {
        record_history(state_dir, profile_id, result);
    }
    outcome
}

#[allow(clippy::too_many_arguments)]
fn run_supervised(
    plan: &ConnectionPlan,
    profile_id: ProfileId,
    session: SessionId,
    store: &impl CredentialStore,
    helper: &Path,
    config_root: &Path,
    records_dir: &Path,
    state_dir: &Path,
    preflight_timeout: Duration,
    supervisor: ProcessIdentity,
) -> Result<SessionResult, SuperviseError> {
    let mut record = SessionRecord {
        session_id: session,
        profile_id,
        supervisor,
        freerdp: None,
        tunnel: None,
        state: SessionRecordState::Preparing,
    };
    record::write(records_dir, &record).map_err(SuperviseError::Io)?;

    let lease = acquire(store, plan.credentials).map_err(SuperviseError::Credential)?;
    let askpass =
        AskpassLease::prepare(&lease, helper.to_path_buf()).map_err(SuperviseError::Io)?;

    let mut prepared = prepare_for_session(plan, session)
        .map_err(PreflightError::Preparation)
        .map_err(SuperviseError::Preflight)?;
    let outcome = (|| {
        verify_prepared(&prepared, preflight_timeout).map_err(SuperviseError::Preflight)?;
        record.tunnel = tunnel_identity(&prepared, session);

        let started = Instant::now();
        let log = SessionLog::new(records_dir.join(format!("{session}.log")));
        let mut child = launch(
            &prepared,
            session,
            Some(&askpass),
            LaunchMode::OneShot,
            Some(log.path()),
        )
        .map_err(SuperviseError::Io)?;

        record.freerdp = Some(child.identity);
        record.state = SessionRecordState::Running;
        record::write(records_dir, &record).map_err(SuperviseError::Io)?;

        let mut monitor_outcome = monitor(&mut child, log.path()).map_err(SuperviseError::Io)?;

        // X11 fallback (parity with Python `should_fallback_to_x11`): the
        // experimental SDL client exited non-zero before it could map a window on a
        // fullscreen profile. Retry once with the stable X11 client, reusing the
        // already-acquired route and askpass. Gated on the X11 client actually being
        // installed (freerdp.capabilities.x11).
        let exited_nonzero =
            matches!(&monitor_outcome, MonitorOutcome::Exited(status) if !status.success());
        if should_fallback_to_x11(
            prepared.plan.client.renderer,
            prepared.plan.display.fullscreen,
            exited_nonzero,
            started.elapsed(),
        ) && let Some(x11) = discover_x11_client()
        {
            prepared.plan.client = x11;
            let mut fallback = launch(
                &prepared,
                session,
                Some(&askpass),
                LaunchMode::OneShot,
                Some(log.path()),
            )
            .map_err(SuperviseError::Io)?;
            record.freerdp = Some(fallback.identity);
            record::write(records_dir, &record).map_err(SuperviseError::Io)?;
            monitor_outcome = monitor(&mut fallback, log.path()).map_err(SuperviseError::Io)?;
        }

        record.state = SessionRecordState::Ending;
        record::write(records_dir, &record).map_err(SuperviseError::Io)?;

        result_from_monitor(
            monitor_outcome,
            &prepared,
            profile_id,
            config_root,
            state_dir,
            started.elapsed(),
        )
    })();

    let cleanup = if let Some(RouteHandle::SshTunnel(handle)) = &mut prepared.route_handle {
        terminate(handle).map_err(SuperviseError::Io)
    } else {
        Ok(())
    };
    match (outcome, cleanup) {
        (Ok(result), Ok(())) => Ok(result),
        (Err(error), _) | (Ok(_), Err(error)) => Err(error),
    }
}

fn result_from_monitor(
    outcome: MonitorOutcome,
    prepared: &PreparedConnection,
    profile_id: ProfileId,
    config_root: &Path,
    state_dir: &Path,
    duration: Duration,
) -> Result<SessionResult, SuperviseError> {
    let (exit_code, failure) = match outcome {
        MonitorOutcome::Exited(status) => {
            let _ = crate::freerdp::certificate::remove_mismatch(state_dir, profile_id);
            (
                status.code(),
                (!status.success()).then_some(ConnectionFailure::ProcessFailure),
            )
        }
        MonitorOutcome::CertificateChanged(presented_sha256) => {
            let endpoint = prepared.plan.target.clone();
            let pin = crate::freerdp::certificate::pin_path(
                &crate::paths::freerdp_config_root(config_root),
                &endpoint.host.to_string(),
                endpoint.port,
            );
            let mismatch = crate::freerdp::certificate::CertificateMismatch {
                profile_id,
                endpoint,
                pinned_sha256: crate::freerdp::certificate::fingerprint(&pin)
                    .map_err(SuperviseError::Io)?,
                presented_sha256,
            };
            crate::freerdp::certificate::write_mismatch(state_dir, &mismatch)
                .map_err(SuperviseError::Io)?;
            (None, Some(ConnectionFailure::Certificate))
        }
    };
    Ok(SessionResult {
        duration,
        exit_code,
        failure,
        renderer: prepared.plan.client.renderer,
        freerdp_version: prepared.plan.client.version.clone(),
    })
}

/// Decide whether a finished attempt should be retried on X11. True only for an
/// SDL fullscreen profile whose client exited non-zero inside [`MAP_GRACE`] — an
/// early failure that never produced a usable window (a certificate change or a
/// later exit is a real session, not a broken renderer).
fn should_fallback_to_x11(
    renderer: Renderer,
    fullscreen: bool,
    exited_nonzero: bool,
    elapsed: Duration,
) -> bool {
    matches!(renderer, Renderer::WaylandSdl) && fullscreen && exited_nonzero && elapsed < MAP_GRACE
}

/// Discover the installed stable X11 client, or `None` when X11 is unavailable.
fn discover_x11_client() -> Option<FreeRdpClient> {
    crate::freerdp::discover::discover(Renderer::X11)
        .ok()
        .map(|discovered| discovered.client)
}

enum MonitorOutcome {
    Exited(std::process::ExitStatus),
    CertificateChanged(String),
}

struct SessionLog(std::path::PathBuf);

impl SessionLog {
    fn new(path: std::path::PathBuf) -> Self {
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for SessionLog {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Wait for `FreeRDP`, interrupting a hidden changed-certificate prompt if one
/// appears in its output rather than hanging on it indefinitely (certificate
/// contract `changed_certificate` detection). Only the identity-verified child
/// is terminated.
fn monitor(child: &mut OwnedChild, log: &Path) -> std::io::Result<MonitorOutcome> {
    const POLL: Duration = Duration::from_millis(100);
    const TAIL_LIMIT: usize = 64 * 1024;
    let mut reader = std::fs::File::open(log)?;
    let mut tail = Vec::new();
    loop {
        reader.read_to_end(&mut tail)?;
        if let Some(fingerprint) =
            crate::freerdp::certificate::changed_fingerprint(&String::from_utf8_lossy(&tail))
        {
            child.terminate_if_owned()?;
            return Ok(MonitorOutcome::CertificateChanged(fingerprint));
        }
        if tail.len() > TAIL_LIMIT {
            tail.drain(..tail.len() - TAIL_LIMIT);
        }
        if let Some(status) = child.child.try_wait()? {
            reader.read_to_end(&mut tail)?;
            if let Some(fingerprint) =
                crate::freerdp::certificate::changed_fingerprint(&String::from_utf8_lossy(&tail))
            {
                return Ok(MonitorOutcome::CertificateChanged(fingerprint));
            }
            return Ok(MonitorOutcome::Exited(status));
        }
        std::thread::sleep(POLL);
    }
}

fn tunnel_identity(prepared: &PreparedConnection, session: SessionId) -> Option<ProcessIdentity> {
    match &prepared.route_handle {
        Some(RouteHandle::SshTunnel(handle)) => {
            observe(handle.child.id(), ChildKind::Tunnel, session).ok()
        }
        None => None,
    }
}

/// Append this session's outcome to the connection history. Best-effort: a
/// history write must never fail a session that already completed.
fn record_history(state_dir: &Path, profile_id: ProfileId, result: &SessionResult) {
    const HISTORY_CAP: usize = 200;
    let finished_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let entry = HistoryEntry::from_result(profile_id, result, finished_at);
    let _ = ConfigStore::new(state_dir).record_history(entry, HISTORY_CAP);
}

#[cfg(test)]
mod tests {
    use super::{MAP_GRACE, Renderer, should_fallback_to_x11};
    use std::time::Duration;

    #[test]
    fn falls_back_when_sdl_fullscreen_fails_before_mapping() {
        assert!(should_fallback_to_x11(
            Renderer::WaylandSdl,
            true,
            true,
            Duration::from_millis(200)
        ));
    }

    #[test]
    fn no_fallback_when_the_session_lived_past_the_map_grace() {
        assert!(!should_fallback_to_x11(
            Renderer::WaylandSdl,
            true,
            true,
            MAP_GRACE + Duration::from_secs(1)
        ));
    }

    #[test]
    fn no_fallback_for_a_clean_exit() {
        assert!(!should_fallback_to_x11(
            Renderer::WaylandSdl,
            true,
            false,
            Duration::from_millis(200)
        ));
    }

    #[test]
    fn no_fallback_for_a_windowed_profile() {
        assert!(!should_fallback_to_x11(
            Renderer::WaylandSdl,
            false,
            true,
            Duration::from_millis(200)
        ));
    }

    #[test]
    fn no_fallback_when_already_on_x11() {
        assert!(!should_fallback_to_x11(
            Renderer::X11,
            true,
            true,
            Duration::from_millis(200)
        ));
    }
}
