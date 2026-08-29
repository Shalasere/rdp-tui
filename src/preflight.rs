//! Explicit conversion from a pure plan into a prepared connection.

use crate::model::{ConnectionFailure, ConnectionPlan, PlannedRoute, PreparedConnection};
use crate::ssh::tunnel::establish;
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

/// Prepare a connection and verify the endpoint reachable from this host.
///
/// A gateway route is checked at its gateway endpoint only. Its target may be
/// intentionally resolvable only from inside the gateway network, so local
/// target DNS and TCP checks would incorrectly reject a valid profile.
///
/// # Errors
///
/// Returns preparation failures or the DNS, timeout, and network failures
/// reported by [`check_tcp`].
pub fn preflight(
    plan: &ConnectionPlan,
    timeout: Duration,
) -> Result<PreparedConnection, ConnectionFailure> {
    let prepared = prepare(plan)?;
    let endpoint = match &plan.route {
        PlannedRoute::Direct => &prepared.effective_endpoint,
        PlannedRoute::RdGateway { gateway } => gateway,
        PlannedRoute::SshTunnel { .. } => unreachable!("prepare rejects unsupported SSH routes"),
    };
    check_tcp(endpoint, timeout)?;
    Ok(prepared)
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

/// Prepare a route for one session, retaining an SSH tunnel when required.
///
/// # Errors
///
/// Returns [`ConnectionFailure::Tunnel`] when the retained SSH forward cannot
/// be established, otherwise the same errors as [`prepare`].
pub fn prepare_for_session(
    plan: &ConnectionPlan,
    session: crate::model::SessionId,
) -> Result<PreparedConnection, ConnectionFailure> {
    let PlannedRoute::SshTunnel { jump_host, target } = &plan.route else {
        return prepare(plan);
    };
    let tunnel = establish(jump_host, target, session).map_err(|_| ConnectionFailure::Tunnel)?;
    Ok(PreparedConnection {
        plan: plan.clone(),
        effective_endpoint: tunnel.local_endpoint.clone(),
        route_handle: Some(crate::model::RouteHandle::SshTunnel(tunnel)),
    })
}
