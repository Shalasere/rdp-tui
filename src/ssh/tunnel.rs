//! SSH local-forward command construction, startup, and identity-safe cleanup.

use crate::model::{Endpoint, TunnelHandle};
use crate::runtime::process::spawn_child;
use crate::runtime::registry::{ChildKind, ProcessIdentity, still_matches};
use std::ffi::OsString;
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

const STARTUP_ATTEMPTS: usize = 20;
const STARTUP_DELAY: Duration = Duration::from_millis(25);

static IDENTITIES: OnceLock<Mutex<std::collections::HashMap<(u32, Instant), ProcessIdentity>>> =
    OnceLock::new();

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

/// SSH startup or local-forward failure without secret material.
#[derive(Debug)]
pub enum TunnelError {
    Spawn(std::io::Error),
    LocalEndpoint(crate::model::EndpointParseError),
    Exited,
    ListenerUnavailable,
}

impl std::fmt::Display for TunnelError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn(error) => write!(formatter, "could not start SSH tunnel: {error}"),
            Self::LocalEndpoint(error) => {
                write!(formatter, "could not allocate tunnel endpoint: {error}")
            }
            Self::Exited => {
                formatter.write_str("SSH tunnel exited before its local forward was ready")
            }
            Self::ListenerUnavailable => {
                formatter.write_str("SSH tunnel did not make its local forward available")
            }
        }
    }
}

impl std::error::Error for TunnelError {}

/// Establish one retained local SSH forward for a connection session.
///
/// # Errors
///
/// Returns a redacted startup or listener error. A lost ephemeral-port race is
/// reported as a clean listener failure because SSH uses `ExitOnForwardFailure`.
pub fn establish(
    jump_host: &str,
    target: &Endpoint,
    session: crate::model::SessionId,
) -> Result<TunnelHandle, TunnelError> {
    establish_with("ssh", jump_host, target, session)
}

/// Terminate and reap a tunnel only after its compound identity still matches.
///
/// # Errors
///
/// Returns an I/O error only when the verified child cannot be terminated or
/// reaped. A missing or mismatched identity never causes a kill attempt.
pub fn terminate(handle: &mut TunnelHandle) -> std::io::Result<()> {
    let Some(identity) = lock_identities()?.remove(&(handle.child.id(), handle.established_at))
    else {
        return Ok(());
    };
    if still_matches(identity) {
        handle.child.kill()?;
        let _ = handle.child.wait()?;
    }
    Ok(())
}

fn establish_with(
    executable: &str,
    jump_host: &str,
    target: &Endpoint,
    session: crate::model::SessionId,
) -> Result<TunnelHandle, TunnelError> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(TunnelError::Spawn)?;
    let port = listener.local_addr().map_err(TunnelError::Spawn)?.port();
    drop(listener);
    let local_endpoint = format!("127.0.0.1:{port}")
        .parse()
        .map_err(TunnelError::LocalEndpoint)?;
    let mut process = Command::new(executable);
    process.args(command(jump_host, port, target));
    let child =
        spawn_child(&mut process, ChildKind::Tunnel, session).map_err(TunnelError::Spawn)?;
    let established_at = Instant::now();
    lock_identities()
        .map_err(TunnelError::Spawn)?
        .insert((child.identity.pid, established_at), child.identity);
    let mut handle = TunnelHandle {
        child: child.child,
        local_endpoint,
        established_at,
    };
    match wait_for_listener(&mut handle) {
        Ok(()) => Ok(handle),
        Err(error) => {
            let _ = terminate(&mut handle);
            Err(error)
        }
    }
}

fn wait_for_listener(handle: &mut TunnelHandle) -> Result<(), TunnelError> {
    for _ in 0..STARTUP_ATTEMPTS {
        if handle
            .child
            .try_wait()
            .map_err(TunnelError::Spawn)?
            .is_some()
        {
            return Err(TunnelError::Exited);
        }
        if TcpStream::connect_timeout(
            &handle
                .local_endpoint
                .to_string()
                .parse()
                .expect("local tunnel endpoint is a socket address"),
            STARTUP_DELAY,
        )
        .is_ok()
        {
            return Ok(());
        }
        std::thread::sleep(STARTUP_DELAY);
    }
    Err(TunnelError::ListenerUnavailable)
}

fn identities() -> &'static Mutex<std::collections::HashMap<(u32, Instant), ProcessIdentity>> {
    IDENTITIES.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

fn lock_identities()
-> std::io::Result<MutexGuard<'static, std::collections::HashMap<(u32, Instant), ProcessIdentity>>>
{
    identities()
        .lock()
        .map_err(|_| std::io::Error::other("SSH tunnel identity registry is unavailable"))
}

#[cfg(test)]
mod tests {
    use super::establish_with;
    use crate::model::SessionId;

    #[test]
    fn exited_ssh_is_reported_without_leaking_a_handle() {
        let error = establish_with(
            "false",
            "jump",
            &"anima:3389".parse().unwrap(),
            "550e8400-e29b-41d4-a716-446655440000"
                .parse::<SessionId>()
                .unwrap(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("exited"));
    }
}
