//! Explicit conversion from a pure plan into a prepared connection.

use crate::model::{ConnectionFailure, ConnectionPlan, PlannedRoute, PreparedConnection};
use crate::runtime::process::LaunchMode;
use crate::ssh::tunnel::establish;
use std::fmt;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

/// A failed TCP reachability attempt, retained for user-facing diagnostics.
#[derive(Debug)]
pub struct SocketAttempt {
    address: SocketAddr,
    error: std::io::Error,
}

impl fmt::Display for SocketAttempt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.address, self.error)
    }
}

/// A reachability failure with the endpoint and kernel errors that caused it.
#[derive(Debug)]
pub struct ReachabilityError {
    failure: ConnectionFailure,
    endpoint: crate::model::Endpoint,
    detail: ReachabilityDetail,
}

#[derive(Debug)]
enum ReachabilityDetail {
    Resolution(std::io::Error),
    Attempts(Vec<SocketAttempt>),
}

impl ReachabilityError {
    /// The stable, coarse failure category used by history and callers.
    #[must_use]
    pub const fn failure(&self) -> ConnectionFailure {
        self.failure
    }
}

impl fmt::Display for ReachabilityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} ({:?}): ", self.endpoint, self.failure)?;
        match &self.detail {
            ReachabilityDetail::Resolution(error) => {
                write!(formatter, "DNS lookup failed: {error}")
            }
            ReachabilityDetail::Attempts(attempts) if attempts.is_empty() => {
                formatter.write_str("no socket addresses were returned")
            }
            ReachabilityDetail::Attempts(attempts) => {
                for (index, attempt) in attempts.iter().enumerate() {
                    if index > 0 {
                        formatter.write_str("; ")?;
                    }
                    attempt.fmt(formatter)?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for ReachabilityError {}

/// A preflight failure that preserves both a stable category and diagnostics.
#[derive(Debug)]
pub enum PreflightError {
    Preparation(ConnectionFailure),
    Reachability(ReachabilityError),
}

impl From<ConnectionFailure> for PreflightError {
    fn from(failure: ConnectionFailure) -> Self {
        Self::Preparation(failure)
    }
}

impl fmt::Display for PreflightError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Preparation(failure) => write!(formatter, "{failure:?}"),
            Self::Reachability(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for PreflightError {}

/// Check whether a TCP endpoint can be resolved and reached within `timeout`.
///
/// This intentionally performs no protocol handshake: reachability is the
/// only fact this check can establish without initiating an RDP session.
///
/// # Errors
///
/// Returns a [`ReachabilityError`] with a stable failure category and every
/// failed socket attempt. This prevents caller-facing diagnostics from losing
/// actionable kernel errors such as "Connection refused" or "No route to host".
pub fn check_tcp(
    endpoint: &crate::model::Endpoint,
    timeout: Duration,
) -> Result<(), ReachabilityError> {
    crate::diagnostics::log(format_args!(
        "preflight tcp start endpoint={endpoint} timeout_ms={}",
        timeout.as_millis()
    ));
    let addresses = match endpoint.to_string().to_socket_addrs() {
        Ok(addresses) => addresses,
        Err(source) => {
            let error = ReachabilityError {
                failure: ConnectionFailure::Dns,
                endpoint: endpoint.clone(),
                detail: ReachabilityDetail::Resolution(source),
            };
            crate::diagnostics::log(format_args!("preflight tcp failed: {error}"));
            return Err(error);
        }
    };
    let mut attempts = Vec::new();
    let mut timed_out = false;
    let deadline = Instant::now() + timeout;
    for address in addresses {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            timed_out = true;
            attempts.push(SocketAttempt {
                address,
                error: std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "overall reachability deadline expired",
                ),
            });
            break;
        }
        match TcpStream::connect_timeout(&address, remaining) {
            Ok(_) => return Ok(()),
            Err(error) => {
                timed_out |= error.kind() == std::io::ErrorKind::TimedOut;
                attempts.push(SocketAttempt { address, error });
            }
        }
    }
    let error = ReachabilityError {
        failure: if timed_out && !attempts.is_empty() {
            ConnectionFailure::Timeout
        } else {
            ConnectionFailure::Network
        },
        endpoint: endpoint.clone(),
        detail: ReachabilityDetail::Attempts(attempts),
    };
    crate::diagnostics::log(format_args!("preflight tcp failed: {error}"));
    Err(error)
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
) -> Result<PreparedConnection, PreflightError> {
    let prepared = prepare(plan)?;
    let endpoint = match &plan.route {
        PlannedRoute::Direct => &prepared.effective_endpoint,
        PlannedRoute::RdGateway { gateway, .. } => gateway,
        PlannedRoute::SshTunnel { .. } => unreachable!("prepare rejects unsupported SSH routes"),
    };
    check_tcp(endpoint, timeout).map_err(PreflightError::Reachability)?;
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

/// Verify a prepared connection's reachable endpoint without re-preparing it,
/// so a retained SSH tunnel is reused rather than reacquired (INV-6). A gateway
/// route is checked at its gateway only (AP-5), never the internal target.
///
/// # Errors
///
/// Returns the DNS, timeout, and network failures reported by [`check_tcp`].
pub fn verify_prepared(
    prepared: &PreparedConnection,
    timeout: Duration,
) -> Result<(), PreflightError> {
    let endpoint = match &prepared.plan.route {
        PlannedRoute::Direct | PlannedRoute::SshTunnel { .. } => &prepared.effective_endpoint,
        PlannedRoute::RdGateway { gateway, .. } => gateway,
    };
    check_tcp(endpoint, timeout).map_err(PreflightError::Reachability)
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
    // One-shot until a detached connect supervisor owns the retained tunnel and
    // requests LaunchMode::Detached explicitly.
    let tunnel = establish(jump_host, target, session, LaunchMode::OneShot)
        .map_err(|_| ConnectionFailure::Tunnel)?;
    Ok(PreparedConnection {
        plan: plan.clone(),
        effective_endpoint: tunnel.local_endpoint.clone(),
        route_handle: Some(crate::model::RouteHandle::SshTunnel(tunnel)),
    })
}
