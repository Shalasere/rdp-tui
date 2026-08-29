//! Shared one-shot session execution for future CLI and TUI callers.

use crate::freerdp::process::launch;
use crate::model::{ConnectionFailure, PreparedConnection, SessionId, SessionResult};
use std::time::Instant;

/// Launch a prepared connection and return its observable process result.
///
/// # Errors
///
/// Returns an I/O error when the process cannot be started or waited for.
pub fn run(prepared: &PreparedConnection, session: SessionId) -> std::io::Result<SessionResult> {
    let started = Instant::now();
    let mut child = launch(prepared, session, None)?;
    let status = child.wait()?;
    Ok(SessionResult {
        duration: started.elapsed(),
        exit_code: status.code(),
        failure: (!status.success()).then_some(ConnectionFailure::ProcessFailure),
        renderer: prepared.plan.client.renderer,
        freerdp_version: prepared.plan.client.version.clone(),
    })
}
