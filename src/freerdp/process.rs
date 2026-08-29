//! `FreeRDP` process launch through the shared identity-checked runtime.

use super::command::build_command;
use crate::credentials::askpass::AskpassLease;
use crate::model::{PreparedConnection, SessionId};
use crate::runtime::process::{LaunchMode, OwnedChild, spawn_child};
use crate::runtime::registry::ChildKind;
use std::process::Command;

/// Launch a prepared connection without a shell or credential material in argv.
///
/// # Errors
///
/// Returns an I/O error when the selected client cannot be spawned or captured.
pub fn launch(
    prepared: &PreparedConnection,
    session: SessionId,
    askpass: Option<&AskpassLease>,
    mode: LaunchMode,
) -> std::io::Result<OwnedChild> {
    let (executable, arguments, environment) = build_command(prepared);
    let mut command = Command::new(executable);
    command.args(arguments).envs(environment);
    if let Some(askpass) = askpass {
        command.envs(askpass.environment());
    }
    spawn_child(&mut command, ChildKind::FreeRdp, session, mode)
}
