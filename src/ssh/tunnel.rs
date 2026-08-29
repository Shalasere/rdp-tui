//! Pure SSH local-forward command construction.

use crate::model::Endpoint;
use std::ffi::OsString;

/// Build the non-interactive SSH command for one retained local forward.
#[must_use]
pub fn command(jump_host: &str, local_port: u16, target: &Endpoint) -> Vec<OsString> {
    vec![
        "-N".into(),
        "-T".into(),
        "-o".into(),
        "BatchMode=yes".into(),
        "-o".into(),
        "StrictHostKeyChecking=accept-new".into(),
        "-o".into(),
        "ExitOnForwardFailure=yes".into(),
        "-L".into(),
        format!("127.0.0.1:{local_port}:{target}").into(),
        jump_host.into(),
    ]
}
