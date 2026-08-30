//! Import of existing Python `rdp-tui` profile data without decrypting secrets.

use super::{ProfilesDocument, StoreError};
use crate::model::{
    CertificatePolicy, Endpoint, IdentityConfig, Profile, ProfileId, Renderer, Route,
    SecurityConfig,
};
use serde::Deserialize;

#[derive(Deserialize)]
#[allow(clippy::struct_excessive_bools)] // Mirrors the legacy JSON binding shape.
struct PythonProfile {
    id: String,
    name: String,
    host: String,
    #[serde(default)]
    user: String,
    #[serde(default)]
    domain: String,
    #[serde(default)]
    fullscreen: bool,
    #[serde(default = "clipboard_default")]
    clipboard: bool,
    #[serde(default)]
    audio: bool,
    #[serde(default)]
    microphone: bool,
    #[serde(default)]
    shared_folder: String,
    #[serde(default)]
    resolution: String,
    #[serde(default)]
    dynamic_resolution: bool,
    #[serde(default)]
    multimon: bool,
    #[serde(default)]
    span_monitors: bool,
    #[serde(default)]
    smart_sizing: bool,
    #[serde(default)]
    scale: u16,
    #[serde(default)]
    color_depth: u8,
    #[serde(default)]
    certificate_policy: String,
    #[serde(default)]
    admin_session: bool,
    #[serde(default)]
    network_type: String,
    #[serde(default)]
    renderer: String,
    #[serde(default)]
    gateway_host: String,
    #[serde(default)]
    ssh_tunnel_host: String,
    #[serde(default)]
    extra_options: String,
}
const fn clipboard_default() -> bool {
    true
}

/// Convert Python `profiles.json` text to validated Rust profiles without reading secrets.
///
/// # Errors
///
/// Returns a schema error for malformed JSON or fields that cannot be represented safely.
pub fn import_python_profiles(text: &str) -> Result<ProfilesDocument, StoreError> {
    let source: Vec<PythonProfile> = serde_json::from_str(text)
        .map_err(|error| schema(0, "profiles.json", error.to_string()))?;
    let mut profiles = Vec::new();
    for (index, source) in source.into_iter().enumerate() {
        profiles.push(convert(index, source)?);
    }
    let document = ProfilesDocument {
        version: 1,
        profiles,
    };
    super::parse_profiles_document(
        &toml::to_string_pretty(&document)
            .map_err(|error| schema(0, "profiles.toml", error.to_string()))?,
    )
}
fn convert(index: usize, source: PythonProfile) -> Result<Profile, StoreError> {
    let id = source
        .id
        .parse::<ProfileId>()
        .map_err(|error| schema(index, "id", error.to_string()))?;
    let endpoint = source
        .host
        .parse::<Endpoint>()
        .map_err(|error| schema(index, "host", error.to_string()))?;
    let mut display = crate::model::DisplayConfig {
        fullscreen: source.fullscreen,
        dynamic_resolution: source.dynamic_resolution,
        multimon: source.multimon,
        span_monitors: source.span_monitors,
        smart_sizing: source.smart_sizing,
        scale_percent: (source.scale != 0).then_some(source.scale),
        color_depth: (source.color_depth != 0).then_some(source.color_depth),
        graphics: crate::model::GraphicsMode::Auto,
        renderer: if source.renderer == "x11" {
            Renderer::X11
        } else {
            Renderer::WaylandSdl
        },
        resolution: parse_resolution(index, &source.resolution)?,
    };
    if source.renderer.is_empty() {
        display.renderer = Renderer::WaylandSdl;
    }
    let route = if !source.ssh_tunnel_host.is_empty() {
        Route::SshTunnel {
            jump_host: source.ssh_tunnel_host,
        }
    } else if !source.gateway_host.is_empty() {
        Route::RdGateway {
            gateway: source
                .gateway_host
                .parse::<Endpoint>()
                .map_err(|error| schema(index, "gateway_host", error.to_string()))?,
            credential: None,
        }
    } else {
        Route::Direct
    };
    let security = SecurityConfig {
        certificate_policy: match source.certificate_policy.as_str() {
            "ignore" => CertificatePolicy::Ignore,
            "deny" => CertificatePolicy::Deny,
            "default" => CertificatePolicy::System,
            _ => CertificatePolicy::Tofu,
        },
        admin_session: source.admin_session,
        network_profile: match source.network_type.as_str() {
            "modem" => crate::model::NetworkProfile::Modem,
            "broadband-low" => crate::model::NetworkProfile::BroadbandLow,
            "broadband-high" => crate::model::NetworkProfile::BroadbandHigh,
            "wan" => crate::model::NetworkProfile::Wan,
            "lan" => crate::model::NetworkProfile::Lan,
            _ => crate::model::NetworkProfile::Auto,
        },
        advanced: crate::model::AdvancedOverrides {
            freerdp_args: if source.extra_options.is_empty() {
                Vec::new()
            } else {
                return Err(schema(
                    index,
                    "extra_options",
                    "manual review required for Python extra options".into(),
                ));
            },
        },
    };
    Ok(Profile {
        id,
        name: source.name,
        endpoint,
        identity: IdentityConfig {
            username: source.user,
            domain: source.domain,
        },
        route,
        display,
        devices: crate::model::DeviceConfig {
            clipboard: source.clipboard,
            audio_playback: source.audio,
            microphone: source.microphone,
            shared_folders: if source.shared_folder.is_empty() {
                Vec::new()
            } else {
                vec![source.shared_folder.into()]
            },
            printers: false,
        },
        security,
        credential: None,
    })
}
fn parse_resolution(index: usize, value: &str) -> Result<Option<(u16, u16)>, StoreError> {
    if value.is_empty() {
        return Ok(None);
    }
    let (w, h) = value
        .split_once('x')
        .ok_or_else(|| schema(index, "resolution", "must be WIDTHxHEIGHT".into()))?;
    Ok(Some((
        w.parse()
            .map_err(|_| schema(index, "resolution", "invalid width".into()))?,
        h.parse()
            .map_err(|_| schema(index, "resolution", "invalid height".into()))?,
    )))
}
fn schema(index: usize, path: &str, found: String) -> StoreError {
    StoreError::Schema {
        file: "profiles.json".into(),
        path: format!("profiles[{index}].{path}"),
        expected: "migratable Python profile field".into(),
        found,
    }
}
