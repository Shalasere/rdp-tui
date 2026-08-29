//! Explicit conversion from a pure plan into a prepared connection.

use crate::model::{ConnectionFailure, ConnectionPlan, PlannedRoute, PreparedConnection};

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
