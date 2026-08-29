//! Detached connect-session records under `$XDG_RUNTIME_DIR/rdp-tui/sessions`.
//!
//! One JSON file per live session lets `doctor` and a restarted launcher see a
//! session the current process did not spawn. See the `session_supervisor`
//! `runtime_record` contract in `docs/architecture/04-amendments.yaml`.

use crate::model::{ProfileId, SessionId};
use crate::runtime::registry::ProcessIdentity;
use serde::{Deserialize, Serialize};
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// Lifecycle state of a detached connect session as seen from its record.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionRecordState {
    /// The supervisor is resolving credentials, preparing, and preflighting.
    Preparing,
    /// `FreeRDP` is running under the supervisor.
    Running,
    /// `FreeRDP` has exited and the supervisor is reaping the route.
    Ending,
}

/// One detached session the supervisor owns, keyed by its session id.
///
/// Every process reference uses the compound `(pid, start_time_ticks, uid)`
/// [`ProcessIdentity`] so a reader never signals a recycled PID (INV-11).
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionRecord {
    pub session_id: SessionId,
    pub profile_id: ProfileId,
    pub supervisor: ProcessIdentity,
    pub freerdp: Option<ProcessIdentity>,
    pub tunnel: Option<ProcessIdentity>,
    pub state: SessionRecordState,
}

/// Resolve `$XDG_RUNTIME_DIR/rdp-tui/sessions`, the per-user session directory.
#[must_use]
pub fn sessions_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(|base| PathBuf::from(base).join("rdp-tui").join("sessions"))
}

fn record_path(dir: &Path, session: SessionId) -> PathBuf {
    dir.join(format!("{session}.json"))
}

/// Durably create or replace one session record in `dir`.
///
/// # Errors
///
/// Returns an I/O error when the directory cannot be created or the record
/// cannot be serialized or atomically replaced.
pub fn write(dir: &Path, record: &SessionRecord) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let json = serde_json::to_vec_pretty(record).map_err(std::io::Error::other)?;
    let mut temporary = tempfile::NamedTempFile::new_in(dir)?;
    temporary.write_all(&json)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(record_path(dir, record.session_id))
        .map_err(|error| error.error)?;
    Ok(())
}

/// Read one session record, returning `None` when it does not exist.
///
/// # Errors
///
/// Returns an I/O error when the record exists but cannot be read or parsed.
pub fn read(dir: &Path, session: SessionId) -> std::io::Result<Option<SessionRecord>> {
    match std::fs::read(record_path(dir, session)) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(std::io::Error::other),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Remove one session record; a missing record is not an error.
///
/// # Errors
///
/// Returns an I/O error when an existing record cannot be removed.
pub fn remove(dir: &Path, session: SessionId) -> std::io::Result<()> {
    match std::fs::remove_file(record_path(dir, session)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Read every session record in `dir`, skipping unreadable or malformed files.
/// A missing directory yields an empty list.
///
/// # Errors
///
/// Returns an I/O error only when the directory exists but cannot be listed.
pub fn list(dir: &Path) -> std::io::Result<Vec<SessionRecord>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut records = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if path.extension().and_then(std::ffi::OsStr::to_str) == Some("json")
            && let Ok(bytes) = std::fs::read(&path)
            && let Ok(record) = serde_json::from_slice::<SessionRecord>(&bytes)
        {
            records.push(record);
        }
    }
    Ok(records)
}
