//! Detached connect-session owner: the logic that runs inside the supervisor
//! process once the launcher has handed it a plan. See the `session_supervisor`
//! contract in `docs/architecture/04-amendments.yaml`.

use crate::credentials::askpass::AskpassLease;
use crate::credentials::{CredentialError, CredentialStore, acquire};
use crate::freerdp::process::launch;
use crate::model::{
    ConnectionFailure, ConnectionPlan, PreparedConnection, ProfileId, RouteHandle, SessionId,
    SessionResult,
};
use crate::preflight::{prepare_for_session, verify_prepared};
use crate::runtime::process::{LaunchMode, OwnedChild};
use crate::runtime::registry::{ChildKind, ProcessIdentity, observe};
use crate::session::record::{self, SessionRecord, SessionRecordState};
use crate::ssh::tunnel::terminate;
use std::path::Path;
use std::time::{Duration, Instant};

/// Failure while supervising a detached connect session.
#[derive(Debug)]
pub enum SuperviseError {
    Credential(CredentialError),
    Preflight(ConnectionFailure),
    Io(std::io::Error),
}

impl std::fmt::Display for SuperviseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Credential(error) => write!(formatter, "credential acquisition failed: {error}"),
            Self::Preflight(failure) => write!(formatter, "preflight failed: {failure:?}"),
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
pub fn supervise(
    plan: &ConnectionPlan,
    profile_id: ProfileId,
    session: SessionId,
    store: &impl CredentialStore,
    helper: &Path,
    records_dir: &Path,
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
        records_dir,
        preflight_timeout,
        supervisor,
    );
    let _ = record::remove(records_dir, session);
    outcome
}

#[allow(clippy::too_many_arguments)]
fn run_supervised(
    plan: &ConnectionPlan,
    profile_id: ProfileId,
    session: SessionId,
    store: &impl CredentialStore,
    helper: &Path,
    records_dir: &Path,
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

    let mut prepared = prepare_for_session(plan, session).map_err(SuperviseError::Preflight)?;
    verify_prepared(&prepared, preflight_timeout).map_err(SuperviseError::Preflight)?;
    record.tunnel = tunnel_identity(&prepared, session);

    let started = Instant::now();
    let log = records_dir.join(format!("{session}.log"));
    let mut child = launch(
        &prepared,
        session,
        Some(&askpass),
        LaunchMode::OneShot,
        Some(&log),
    )
    .map_err(SuperviseError::Io)?;

    record.freerdp = Some(child.identity);
    record.state = SessionRecordState::Running;
    record::write(records_dir, &record).map_err(SuperviseError::Io)?;

    let outcome = monitor(&mut child, &log).map_err(SuperviseError::Io)?;

    record.state = SessionRecordState::Ending;
    record::write(records_dir, &record).map_err(SuperviseError::Io)?;

    if let Some(RouteHandle::SshTunnel(handle)) = &mut prepared.route_handle {
        terminate(handle).map_err(SuperviseError::Io)?;
    }

    let (exit_code, failure) = match outcome {
        MonitorOutcome::Exited(status) => (
            status.code(),
            (!status.success()).then_some(ConnectionFailure::ProcessFailure),
        ),
        MonitorOutcome::CertificateChanged => (None, Some(ConnectionFailure::Certificate)),
    };
    Ok(SessionResult {
        duration: started.elapsed(),
        exit_code,
        failure,
        renderer: prepared.plan.client.renderer,
        freerdp_version: prepared.plan.client.version.clone(),
    })
}

enum MonitorOutcome {
    Exited(std::process::ExitStatus),
    CertificateChanged,
}

/// Wait for `FreeRDP`, interrupting a hidden changed-certificate prompt if one
/// appears in its output rather than hanging on it indefinitely (certificate
/// contract `changed_certificate` detection). Only the identity-verified child
/// is terminated.
fn monitor(child: &mut OwnedChild, log: &Path) -> std::io::Result<MonitorOutcome> {
    const POLL: Duration = Duration::from_millis(100);
    loop {
        if let Some(status) = child.child.try_wait()? {
            return Ok(MonitorOutcome::Exited(status));
        }
        if std::fs::read_to_string(log)
            .ok()
            .and_then(|output| crate::freerdp::certificate::changed_fingerprint(&output))
            .is_some()
        {
            child.terminate_if_owned()?;
            return Ok(MonitorOutcome::CertificateChanged);
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
