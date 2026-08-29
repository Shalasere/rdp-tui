//! Shared one-shot session execution for future CLI and TUI callers.

pub mod record;
pub mod supervisor;

use crate::credentials::askpass::AskpassLease;
use crate::credentials::{CredentialError, CredentialStore, acquire};
use crate::freerdp::process::launch;
use crate::model::{ConnectionFailure, PreparedConnection, SessionId, SessionResult};
use crate::runtime::process::LaunchMode;
use crate::ssh::tunnel::terminate;
use std::path::Path;
use std::time::Instant;

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
    if let Some(crate::model::RouteHandle::SshTunnel(tunnel)) = &mut prepared.route_handle {
        terminate(tunnel)?;
    }
    result
}
