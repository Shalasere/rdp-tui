//! Launcher/supervisor process boundary.
//!
//! [`spawn_supervisor`] starts a detached supervisor and hands it a
//! [`ConnectionPlan`] over an inherited anonymous pipe — never through argv or
//! an ordinary environment value, only the pipe's descriptor number — mirroring
//! the sealed-fd askpass bridge. The plan is length-prefixed so the reader needs
//! no EOF. See the `session_supervisor` contract in
//! `docs/architecture/04-amendments.yaml`.

use crate::model::{ConnectionPlan, ProfileId, SessionId};
use crate::runtime::process::{LaunchMode, spawn_child};
use crate::runtime::registry::ChildKind;
use crate::session::record;
use crate::session::supervisor::supervise;
use std::fs::File;
use std::io::{Read as _, Write as _};
use std::os::fd::{AsRawFd as _, FromRawFd as _, RawFd};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const PLAN_FD: &str = "RDP_TUI_PLAN_FD";
const SESSION_ID: &str = "RDP_TUI_SESSION_ID";
const PROFILE_ID: &str = "RDP_TUI_PROFILE_ID";
const SUPERVISE_ARG: &str = "__supervise";
const PREFLIGHT_TIMEOUT: Duration = Duration::from_secs(10);

/// Spawn a detached supervisor for `plan` and hand it the plan over a pipe.
///
/// The plan and identifiers never appear in argv or ordinary environment
/// values; only the inherited pipe's descriptor number does.
///
/// # Errors
///
/// Returns an I/O error when the pipe, child process, or plan transfer fails.
pub fn spawn_supervisor(
    plan: &ConnectionPlan,
    profile_id: ProfileId,
    session: SessionId,
    executable: &Path,
) -> std::io::Result<()> {
    let (read_end, write_end) = rustix::pipe::pipe()?;
    let mut command = Command::new(executable);
    command.arg(SUPERVISE_ARG);
    command.env(PLAN_FD, read_end.as_raw_fd().to_string());
    command.env(SESSION_ID, session.to_string());
    command.env(PROFILE_ID, profile_id.to_string());
    // The supervisor must outlive this launcher: detached, no parent-death signal.
    let _child = spawn_child(
        &mut command,
        ChildKind::Supervisor,
        session,
        LaunchMode::Detached,
    )?;
    // Hand over the plan length-prefixed, then close our write end. The reader
    // takes the byte count from the prefix, so it never depends on EOF.
    let json = serde_json::to_vec(plan).map_err(std::io::Error::other)?;
    let length = u32::try_from(json.len())
        .map_err(|_| std::io::Error::other("connection plan too large"))?;
    let mut writer = File::from(write_end);
    writer.write_all(&length.to_le_bytes())?;
    writer.write_all(&json)?;
    writer.flush()?;
    drop(writer);
    drop(read_end);
    Ok(())
}

/// Supervisor entry point: read the inherited plan and run the session.
///
/// # Errors
///
/// Returns an error when the environment handoff is missing or malformed, or
/// when supervising the session fails.
pub fn run_from_environment() -> std::io::Result<()> {
    let fd: RawFd = env_var(PLAN_FD)?
        .parse()
        .map_err(|_| std::io::Error::other("invalid plan descriptor"))?;
    let session: SessionId = env_var(SESSION_ID)?
        .parse()
        .map_err(|_| std::io::Error::other("invalid supervised session id"))?;
    let profile_id: ProfileId = env_var(PROFILE_ID)?
        .parse()
        .map_err(|_| std::io::Error::other("invalid supervised profile id"))?;
    let plan = read_plan(fd)?;
    let store = crate::credentials::SystemCredentialStore::new(config_root());
    let helper = std::env::current_exe()?;
    let records = record::sessions_dir()
        .unwrap_or_else(|| std::env::temp_dir().join("rdp-tui").join("sessions"));
    let state = state_dir();
    supervise(
        &plan,
        profile_id,
        session,
        &store,
        &helper,
        &records,
        &state,
        PREFLIGHT_TIMEOUT,
    )
    .map(|_result| ())
    .map_err(std::io::Error::other)
}

fn env_var(name: &str) -> std::io::Result<String> {
    std::env::var(name)
        .map_err(|_| std::io::Error::other(format!("missing supervised handoff variable {name}")))
}

#[allow(unsafe_code)]
fn read_plan(fd: RawFd) -> std::io::Result<ConnectionPlan> {
    // SAFETY: `fd` is the inherited pipe read end the launcher created solely
    // for this supervisor; we take unique ownership of it here.
    let mut file = unsafe { File::from_raw_fd(fd) };
    let mut length = [0u8; 4];
    file.read_exact(&mut length)?;
    let mut json = vec![0u8; u32::from_le_bytes(length) as usize];
    file.read_exact(&mut json)?;
    serde_json::from_slice(&json).map_err(std::io::Error::other)
}

fn config_root() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"))
        .join("rdp-tui")
}

fn state_dir() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .unwrap_or_else(|| PathBuf::from(".local/state"))
        .join("rdp-tui")
}
