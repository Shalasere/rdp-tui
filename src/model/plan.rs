use super::{
    DeviceConfig, DisplayConfig, Endpoint, IdentityConfig, PlannedRoute, Renderer,
    ResolvedCredentials, SecurityConfig,
};
use semver::Version;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ConnectionPlan {
    pub target: Endpoint,
    pub route: PlannedRoute,
    pub identity: IdentityConfig,
    pub display: DisplayConfig,
    pub devices: DeviceConfig,
    pub security: SecurityConfig,
    pub credentials: ResolvedCredentials,
    pub client: FreeRdpClient,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct FreeRdpClient {
    pub executable: PathBuf,
    pub renderer: Renderer,
    pub version: Version,
}
