use super::{ConnectionPlan, Endpoint};
use std::process::Child;
use std::time::Instant;

#[derive(Debug)]
pub struct PreparedConnection {
    pub plan: ConnectionPlan,
    pub effective_endpoint: Endpoint,
    pub route_handle: Option<RouteHandle>,
}

#[derive(Debug)]
pub enum RouteHandle {
    SshTunnel(TunnelHandle),
}

#[derive(Debug)]
pub struct TunnelHandle {
    pub child: Child,
    pub local_endpoint: Endpoint,
    pub established_at: Instant,
}
