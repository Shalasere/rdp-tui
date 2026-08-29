//! Process spawning with immediate compound identity capture and an explicit
//! detachment policy.

use super::registry::{ChildKind, ProcessIdentity, observe};
use crate::model::SessionId;
use rustix::process::{Signal, set_parent_process_death_signal};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitStatus};

/// Whether a spawned child should outlive the launcher that started it.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum LaunchMode {
    /// A user-facing connect session: the child leads its own process group and
    /// receives no parent-death signal, so closing the launcher does not close
    /// the remote desktop. See DEC-session-detachment.
    Detached,
    /// A one-shot test/preflight child that must never be left behind: it gets
    /// `PR_SET_PDEATHSIG` so the kernel terminates it if the launcher dies.
    ///
    /// Per AP-11 the death signal is tied to the spawning *thread*, so a
    /// one-shot child must be spawned from the main or a dedicated supervisor
    /// thread, never a pooled worker.
    OneShot,
}

/// A child that rdp-tui can identify safely for its lifetime.
#[derive(Debug)]
pub struct OwnedChild {
    pub child: Child,
    pub identity: ProcessIdentity,
}

/// Spawn a child under an explicit detachment policy and capture its
/// PID/start-time/UID identity immediately.
///
/// # Errors
///
/// Returns an I/O error if spawning fails or the child exits before identity capture.
pub fn spawn_child(
    command: &mut Command,
    kind: ChildKind,
    session: SessionId,
    mode: LaunchMode,
) -> std::io::Result<OwnedChild> {
    apply_launch_mode(command, mode);
    let mut child = command.spawn()?;
    match observe(child.id(), kind, session) {
        Ok(identity) => Ok(OwnedChild { child, identity }),
        Err(error) => {
            let _ = child.wait();
            Err(error)
        }
    }
}

fn apply_launch_mode(command: &mut Command, mode: LaunchMode) {
    match mode {
        // A new process group keeps a terminal signal aimed at the launcher's
        // group from reaching a session the user expects to keep running.
        LaunchMode::Detached => {
            command.process_group(0);
        }
        LaunchMode::OneShot => request_parent_death_signal(command),
    }
}

/// Register a pre-exec hook asking the kernel to terminate the child if this
/// launcher dies first. See DEC-session-detachment / INV-13.
#[allow(unsafe_code)]
fn request_parent_death_signal(command: &mut Command) {
    // SAFETY: the closure runs in the forked child before exec and performs a
    // single async-signal-safe prctl(PR_SET_PDEATHSIG) syscall via rustix; it
    // captures nothing, allocates nothing, and touches no shared state.
    unsafe {
        command.pre_exec(|| {
            set_parent_process_death_signal(Some(Signal::TERM)).map_err(std::io::Error::from)
        });
    }
}

impl OwnedChild {
    /// Wait for the owned child to exit.
    ///
    /// # Errors
    ///
    /// Returns an I/O error when waiting fails.
    pub fn wait(&mut self) -> std::io::Result<ExitStatus> {
        self.child.wait()
    }
    /// Terminate the owned child only when its compound identity still matches.
    ///
    /// # Errors
    ///
    /// Returns an I/O error if termination or waiting fails.
    pub fn terminate_if_owned(&mut self) -> std::io::Result<Option<ExitStatus>> {
        if super::registry::still_matches(self.identity) {
            self.child.kill()?;
            self.child.wait().map(Some)
        } else {
            Ok(None)
        }
    }
}
