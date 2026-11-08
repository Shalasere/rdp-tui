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
use crate::session::record::{self, SessionRecord, SessionRecordState};
use crate::session::supervisor::supervise;
use secrecy::zeroize::Zeroizing;
use secrecy::{ExposeSecret as _, SecretString};
use std::fs::File;
use std::io::{Read as _, Write as _};
use std::os::fd::{AsRawFd as _, BorrowedFd, FromRawFd as _, RawFd};
use std::os::unix::process::CommandExt as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

const PLAN_FD: &str = "RDP_TUI_PLAN_FD";
const SESSION_ID: &str = "RDP_TUI_SESSION_ID";
const PROFILE_ID: &str = "RDP_TUI_PROFILE_ID";
const PASSWORD_FD: &str = "RDP_TUI_PASSWORD_FD";
const SUPERVISE_ARG: &str = "__supervise";
const PREFLIGHT_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_PLAN_BYTES: usize = 1024 * 1024;

/// Spawn a detached supervisor for `plan` and hand it the plan over a pipe.
///
/// The plan never appears in argv or ordinary environment values; only the
/// inherited pipe descriptor and non-secret stable identifiers do.
///
/// # Errors
///
/// Returns an I/O error when the pipe, child process, or plan transfer fails.
pub fn spawn_supervisor(
    plan: &ConnectionPlan,
    profile_id: ProfileId,
    session: SessionId,
    executable: &Path,
    password: Option<&SecretString>,
) -> std::io::Result<()> {
    if let Some(password) = password {
        let length = password.expose_secret().len();
        if length == 0 || length > 4096 {
            return Err(std::io::Error::other(
                "password length must be 1–4096 bytes",
            ));
        }
    }
    let (read_end, write_end) = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC)?;
    let password_pipe = password
        .map(|_| rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC))
        .transpose()?;
    let mut command = Command::new(executable);
    command.arg(SUPERVISE_ARG);
    // Detached owners must not read from or write onto the launcher's terminal.
    // Outcomes go through history; opt-in diagnostics use their own log file.
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command.env(PLAN_FD, read_end.as_raw_fd().to_string());
    command.env(SESSION_ID, session.to_string());
    command.env(PROFILE_ID, profile_id.to_string());
    inherit_fd_for_exec(&mut command, read_end.as_raw_fd());
    if let Some((secret_read, _)) = &password_pipe {
        command.env(PASSWORD_FD, secret_read.as_raw_fd().to_string());
        inherit_fd_for_exec(&mut command, secret_read.as_raw_fd());
    }
    crate::diagnostics::configure_command(&mut command);
    // The supervisor must outlive this launcher: detached, no parent-death signal.
    let child = spawn_child(
        &mut command,
        ChildKind::Supervisor,
        session,
        LaunchMode::Detached,
    )?;
    let supervisor = child.identity;
    // The TUI may live for hours after a detached supervisor exits. Keep a
    // waiter attached so the completed supervisor is reaped promptly.
    child.reap_in_background();
    let records = crate::paths::runtime_sessions_dir();
    record::write(
        &records,
        &SessionRecord {
            session_id: session,
            profile_id,
            supervisor,
            freerdp: None,
            tunnel: None,
            state: SessionRecordState::Preparing,
        },
    )?;
    // Hand over the plan length-prefixed, then close our write end. The reader
    // takes the byte count from the prefix, so it never depends on EOF.
    let handoff = (|| {
        let json = serde_json::to_vec(plan).map_err(std::io::Error::other)?;
        let length = u32::try_from(json.len())
            .map_err(|_| std::io::Error::other("connection plan too large"))?;
        let mut writer = File::from(write_end);
        writer.write_all(&length.to_le_bytes())?;
        writer.write_all(&json)?;
        writer.flush()?;
        drop(writer);
        drop(read_end);
        if let (Some(password), Some((secret_read, secret_write))) = (password, password_pipe) {
            let bytes = password.expose_secret().as_bytes();
            let length = u32::try_from(bytes.len())
                .map_err(|_| std::io::Error::other("password is too long"))?;
            let mut writer = File::from(secret_write);
            writer.write_all(&length.to_le_bytes())?;
            writer.write_all(bytes)?;
            writer.flush()?;
            drop(writer);
            drop(secret_read);
        }
        Ok(())
    })();
    if handoff.is_err() {
        let _ = record::remove(&records, session);
    }
    handoff
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
    let password = std::env::var(PASSWORD_FD)
        .ok()
        .map(|fd| {
            let fd = fd
                .parse()
                .map_err(|_| std::io::Error::other("invalid password descriptor"))?;
            read_password(fd)
        })
        .transpose()?;
    let config_root = crate::paths::config_root();
    let store = crate::credentials::SystemCredentialStore::new(&config_root);
    let helper = std::env::current_exe()?;
    let records = crate::paths::runtime_sessions_dir();
    let state = crate::paths::state_root();
    supervise(
        &plan,
        profile_id,
        session,
        &store,
        &helper,
        &config_root,
        &records,
        &state,
        PREFLIGHT_TIMEOUT,
        password,
    )
    .map(|_result| ())
    .map_err(std::io::Error::other)
}

#[allow(unsafe_code)]
fn read_password(fd: RawFd) -> std::io::Result<SecretString> {
    // SAFETY: this is the supervisor's uniquely inherited password pipe.
    let mut file = unsafe { File::from_raw_fd(fd) };
    let mut length = [0u8; 4];
    file.read_exact(&mut length)?;
    let length = u32::from_le_bytes(length) as usize;
    if length == 0 || length > 4096 {
        return Err(std::io::Error::other("invalid password length"));
    }
    let mut bytes = Zeroizing::new(vec![0u8; length]);
    file.read_exact(&mut bytes)?;
    let password =
        std::str::from_utf8(&bytes).map_err(|_| std::io::Error::other("password is not UTF-8"))?;
    Ok(SecretString::from(password.to_owned()))
}

fn env_var(name: &str) -> std::io::Result<String> {
    std::env::var(name)
        .map_err(|_| std::io::Error::other(format!("missing supervised handoff variable {name}")))
}

#[allow(unsafe_code)]
fn inherit_fd_for_exec(command: &mut Command, fd: RawFd) {
    // SAFETY: this hook runs after fork and before exec. It performs only the
    // async-signal-safe fcntl syscall on the still-owned pipe descriptor. The
    // parent retains CLOEXEC, preventing concurrent unrelated spawns from
    // inheriting either end of the handoff pipe.
    unsafe {
        command.pre_exec(move || {
            let borrowed = BorrowedFd::borrow_raw(fd);
            rustix::io::fcntl_setfd(borrowed, rustix::io::FdFlags::empty())
                .map_err(std::io::Error::from)
        });
    }
}

#[allow(unsafe_code)]
fn read_plan(fd: RawFd) -> std::io::Result<ConnectionPlan> {
    // SAFETY: `fd` is the inherited pipe read end the launcher created solely
    // for this supervisor; we take unique ownership of it here.
    let mut file = unsafe { File::from_raw_fd(fd) };
    let mut length = [0u8; 4];
    file.read_exact(&mut length)?;
    let length = u32::from_le_bytes(length) as usize;
    if length == 0 || length > MAX_PLAN_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "supervised connection plan has an invalid length",
        ));
    }
    let mut json = vec![0u8; length];
    file.read_exact(&mut json)?;
    serde_json::from_slice(&json).map_err(std::io::Error::other)
}
