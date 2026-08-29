//! `/proc` process identity that is safe against PID reuse, plus a
//! process-local registry of the children rdp-tui currently owns.

use crate::model::SessionId;
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ChildKind {
    Tunnel,
    FreeRdp,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub start_time_ticks: u64,
    pub uid: u32,
    pub kind: ChildKind,
    pub parent_session_id: SessionId,
}

/// How a registered child compares to what `/proc` currently shows.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ProcessObservedState {
    /// PID, start time, and UID still identify our exact child.
    StillRunning,
    /// The PID is live but its identity differs — it was recycled to another
    /// process and must never be signalled as if it were ours.
    Unowned,
    /// No process with this PID exists; the record is safe to drop.
    Stale,
}

/// A registered child paired with its currently observed state.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct DiscoveredProcess {
    pub identity: ProcessIdentity,
    pub observed_state: ProcessObservedState,
}

/// Read a PID's stable Linux identity fields for a session-owned child.
///
/// # Errors
///
/// Returns an error when the process has exited or `/proc` cannot be read.
pub fn observe(
    pid: u32,
    kind: ChildKind,
    parent_session_id: SessionId,
) -> std::io::Result<ProcessIdentity> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let tail = stat
        .rsplit_once(") ")
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid proc stat"))?
        .1;
    let start_time_ticks = tail
        .split_whitespace()
        .nth(19)
        .ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "missing proc start time")
        })?
        .parse()
        .map_err(|_| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid proc start time")
        })?;
    let uid = fs::metadata(format!("/proc/{pid}"))?.uid();
    Ok(ProcessIdentity {
        pid,
        start_time_ticks,
        uid,
        kind,
        parent_session_id,
    })
}

/// Return true only when PID, start time, and UID still identify this exact child.
#[must_use]
pub fn still_matches(identity: ProcessIdentity) -> bool {
    observe(identity.pid, identity.kind, identity.parent_session_id).is_ok_and(|current| {
        current.pid == identity.pid
            && current.start_time_ticks == identity.start_time_ticks
            && current.uid == identity.uid
    })
}

static OWNED: OnceLock<Mutex<HashMap<u32, ProcessIdentity>>> = OnceLock::new();

fn owned() -> &'static Mutex<HashMap<u32, ProcessIdentity>> {
    OWNED.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock_owned() -> MutexGuard<'static, HashMap<u32, ProcessIdentity>> {
    owned().lock().unwrap_or_else(PoisonError::into_inner)
}

/// Record a spawned child rdp-tui owns. Keyed by PID, which is unique among the
/// live children we have not yet reaped.
pub fn register(identity: ProcessIdentity) {
    lock_owned().insert(identity.pid, identity);
}

/// Remove and return a registered child by PID. The returned value is only a
/// record; callers must still verify [`still_matches`] before acting on it.
#[must_use]
pub fn deregister(pid: u32) -> Option<ProcessIdentity> {
    lock_owned().remove(&pid)
}

/// Classify every registered child against `/proc` without signalling anything.
///
/// A [`ProcessObservedState::Unowned`] entry shares a recycled PID with an
/// unrelated process and must be left untouched; a
/// [`ProcessObservedState::Stale`] entry no longer exists and can be dropped.
#[must_use]
pub fn scan_for_orphans() -> Vec<DiscoveredProcess> {
    lock_owned()
        .values()
        .map(|identity| {
            let observed_state =
                match observe(identity.pid, identity.kind, identity.parent_session_id) {
                    Ok(current) if current == *identity => ProcessObservedState::StillRunning,
                    Ok(_) => ProcessObservedState::Unowned,
                    Err(_) => ProcessObservedState::Stale,
                };
            DiscoveredProcess {
                identity: *identity,
                observed_state,
            }
        })
        .collect()
}
