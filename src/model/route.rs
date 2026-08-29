use super::{CredentialRef, Endpoint};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
pub enum Route {
    #[default]
    Direct,
    RdGateway {
        gateway: Endpoint,
        credential: Option<CredentialRef>,
    },
    SshTunnel {
        jump_host: String,
    },
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", deny_unknown_fields)]
pub enum PlannedRoute {
    Direct,
    RdGateway { gateway: Endpoint },
    SshTunnel { jump_host: String, target: Endpoint },
}
