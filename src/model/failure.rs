use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionFailure {
    Configuration,
    Dns,
    Network,
    Timeout,
    Ssh,
    Tunnel,
    Gateway,
    Authentication,
    Certificate,
    UnsupportedCapability,
    FreeRdpMissing,
    FreeRdpVersion,
    Renderer,
    ProcessFailure,
    Unknown,
}
