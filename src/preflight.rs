//! Explicit conversion from a pure plan into a prepared connection.

use crate::model::{ConnectionFailure, ConnectionPlan, PlannedRoute, PreparedConnection};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

/// Check whether a TCP endpoint can be resolved and reached within `timeout`.
///
/// This intentionally performs no protocol handshake: reachability is the
/// only fact this check can establish without initiating an RDP session.
///
/// # Errors
///
/// Returns [`ConnectionFailure::Dns`] when resolution fails,
/// [`ConnectionFailure::Timeout`] when every attempted address times out, and
/// [`ConnectionFailure::Network`] for an empty resolution or other connection
/// failure.
pub fn check_tcp(
    endpoint: &crate::model::Endpoint,
    timeout: Duration,
) -> Result<(), ConnectionFailure> {
    let addresses = endpoint
        .to_string()
        .to_socket_addrs()
        .map_err(|_| ConnectionFailure::Dns)?;
    let mut attempted = false;
    let mut timed_out = false;
    for address in addresses {
        attempted = true;
        match TcpStream::connect_timeout(&address, timeout) {
            Ok(_) => return Ok(()),
            Err(error) => timed_out |= error.kind() == std::io::ErrorKind::TimedOut,
        }
    }
    if timed_out && attempted {
        Err(ConnectionFailure::Timeout)
    } else {
        Err(ConnectionFailure::Network)
    }
}

/// Prepare direct and RD Gateway plans without acquiring hidden resources.
///
/// SSH tunnels are intentionally rejected until their retained-process
/// lifecycle is implemented; this prevents a fake prepared state.
///
/// # Errors
///
/// Returns `UnsupportedCapability` for SSH routes pending tunnel support.
pub fn prepare(plan: &ConnectionPlan) -> Result<PreparedConnection, ConnectionFailure> {
    if matches!(plan.route, PlannedRoute::SshTunnel { .. }) {
        return Err(ConnectionFailure::UnsupportedCapability);
    }
    Ok(PreparedConnection {
        plan: plan.clone(),
        effective_endpoint: plan.target.clone(),
        route_handle: None,
    })
}
