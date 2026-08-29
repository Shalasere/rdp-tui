//! Process spawning with immediate compound identity capture.

use super::registry::{ChildKind, ProcessIdentity, observe};
use crate::model::SessionId;
use std::process::{Child, Command, ExitStatus};

/// A child that rdp-tui can identify safely for its lifetime.
#[derive(Debug)]
pub struct OwnedChild {
    pub child: Child,
    pub identity: ProcessIdentity,
}

/// Spawn a child and capture its PID/start-time/UID identity immediately.
///
/// # Errors
///
/// Returns an I/O error if spawning fails or the child exits before identity capture.
pub fn spawn_child(
    command: &mut Command,
    kind: ChildKind,
    session: SessionId,
) -> std::io::Result<OwnedChild> {
    let mut child = command.spawn()?;
    match observe(child.id(), kind, session) {
        Ok(identity) => Ok(OwnedChild { child, identity }),
        Err(error) => {
            let _ = child.wait();
            Err(error)
        }
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
