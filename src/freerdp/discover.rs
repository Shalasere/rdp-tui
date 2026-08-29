//! Installed `FreeRDP` frontend discovery and version parsing.

use super::capabilities::{AuthOnlySupport, FreeRdpCapabilities};
use crate::model::{FreeRdpClient, Renderer};
use semver::Version;
use std::path::PathBuf;
use std::process::Command;

/// A discovered frontend paired with its conservative capabilities.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct DiscoveredFreeRdp {
    pub client: FreeRdpClient,
    pub capabilities: FreeRdpCapabilities,
}

/// Discover the requested installed frontend and obtain its semantic version.
///
/// # Errors
///
/// Returns a descriptive error when no frontend is installed, it cannot run,
/// or its version output is not recognized.
pub fn discover(renderer: Renderer) -> Result<DiscoveredFreeRdp, String> {
    let executable =
        find_frontend(renderer).ok_or_else(|| "FreeRDP frontend is not installed".to_owned())?;
    let output = Command::new(&executable)
        .arg("/version")
        .output()
        .map_err(|error| error.to_string())?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let version =
        parse_version(&text).ok_or_else(|| "could not parse FreeRDP version output".to_owned())?;
    let capabilities = FreeRdpCapabilities {
        version: version.clone(),
        sdl: renderer == Renderer::WaylandSdl,
        x11: renderer == Renderer::X11,
        askpass: true,
        gateway: true,
        multimon: true,
        dynamic_resolution: true,
        auth_only: if version.major >= 3 {
            AuthOnlySupport::Validated
        } else {
            AuthOnlySupport::Unvalidated
        },
    };
    Ok(DiscoveredFreeRdp {
        client: FreeRdpClient {
            executable,
            renderer,
            version,
        },
        capabilities,
    })
}

fn find_frontend(renderer: Renderer) -> Option<PathBuf> {
    let names: &[&str] = if renderer == Renderer::WaylandSdl {
        &["sdl-freerdp3"]
    } else {
        &["xfreerdp3", "xfreerdp"]
    };
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .flat_map(|directory| names.iter().map(move |name| directory.join(name)))
        .find(|path| path.is_file())
}

#[must_use]
pub fn parse_version(output: &str) -> Option<Version> {
    output.split_whitespace().find_map(|word| {
        Version::parse(word.trim_matches(|value: char| !value.is_ascii_digit() && value != '.'))
            .ok()
    })
}
