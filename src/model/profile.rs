use super::{CredentialRef, Endpoint, ProfileId, Route};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub id: ProfileId,
    pub name: String,
    pub endpoint: Endpoint,
    pub identity: IdentityConfig,
    pub route: Route,
    pub display: DisplayConfig,
    pub devices: DeviceConfig,
    pub security: SecurityConfig,
    pub credential: Option<CredentialRef>,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct IdentityConfig {
    pub username: String,
    pub domain: String,
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)] // Binding shape is specified by 01-types.yaml.
pub struct DisplayConfig {
    pub renderer: Renderer,
    pub fullscreen: bool,
    pub resolution: Option<(u16, u16)>,
    pub dynamic_resolution: bool,
    pub multimon: bool,
    pub span_monitors: bool,
    pub smart_sizing: bool,
    pub scale_percent: Option<u16>,
    pub color_depth: Option<u8>,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            renderer: Renderer::WaylandSdl,
            fullscreen: true,
            resolution: None,
            dynamic_resolution: false,
            multimon: false,
            span_monitors: false,
            smart_sizing: false,
            scale_percent: None,
            color_depth: None,
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[allow(clippy::struct_excessive_bools)] // Binding shape is specified by 01-types.yaml.
pub struct DeviceConfig {
    pub clipboard: bool,
    pub audio_playback: bool,
    pub microphone: bool,
    pub shared_folders: Vec<PathBuf>,
    pub printers: bool,
}

impl Default for DeviceConfig {
    fn default() -> Self {
        Self {
            clipboard: true,
            audio_playback: true,
            microphone: false,
            shared_folders: Vec::new(),
            printers: false,
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SecurityConfig {
    pub certificate_policy: CertificatePolicy,
    pub admin_session: bool,
    pub network_profile: NetworkProfile,
    pub advanced: AdvancedOverrides,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            certificate_policy: CertificatePolicy::Tofu,
            admin_session: false,
            network_profile: NetworkProfile::Auto,
            advanced: AdvancedOverrides::default(),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct AdvancedOverrides {
    pub freerdp_args: Vec<String>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Renderer {
    #[default]
    WaylandSdl,
    X11,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CertificatePolicy {
    #[default]
    Tofu,
    System,
    Deny,
    Ignore,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum NetworkProfile {
    #[default]
    Auto,
    Modem,
    BroadbandLow,
    BroadbandHigh,
    Wan,
    Lan,
}
