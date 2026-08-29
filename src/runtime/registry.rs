//! `/proc` process identity that is safe against PID reuse.

use crate::model::SessionId;
use std::fs;
use std::os::unix::fs::MetadataExt;

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
