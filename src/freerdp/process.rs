//! `FreeRDP` process launch through the shared identity-checked runtime.

use super::command::build_command;
use crate::credentials::askpass::AskpassLease;
use crate::model::{PreparedConnection, SessionId};
use crate::runtime::process::{LaunchMode, OwnedChild, spawn_child};
use crate::runtime::registry::ChildKind;
use std::path::Path;
use std::process::Command;

/// Launch a prepared connection without a shell or credential material in argv.
///
/// When `log` is set, the child's stdout and stderr are appended to it so a
/// supervisor can watch the output (e.g. for a changed-certificate report)
/// without racing the child on a shared terminal.
///
/// # Errors
///
/// Returns an I/O error when the selected client cannot be spawned or captured,
/// or the log file cannot be opened.
pub fn launch(
    prepared: &PreparedConnection,
    session: SessionId,
    askpass: Option<&AskpassLease>,
    mode: LaunchMode,
    log: Option<&Path>,
) -> std::io::Result<OwnedChild> {
    let (executable, arguments, environment) = build_command(prepared);
    let mut command = Command::new(executable);
    command.args(arguments).envs(environment);
    if let Some(askpass) = askpass {
        command.envs(askpass.environment());
    }
    if let Some(log) = log {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log)?;
        let errors = file.try_clone()?;
        command.stdout(file).stderr(errors);
    }
    spawn_child(&mut command, ChildKind::FreeRdp, session, mode)
}
