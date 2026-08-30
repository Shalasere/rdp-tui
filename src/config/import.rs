//! Credential-free import of Remmina (`.remmina`) and Microsoft (`.rdp`)
//! profiles, and Remmina directories. Passwords are never read. Mirrors the
//! Python client's `profile_io` behavior.

use crate::model::{
    AdvancedOverrides, CertificatePolicy, DeviceConfig, DisplayConfig, Endpoint, IdentityConfig,
    NetworkProfile, Profile, ProfileId, Renderer, Route, SecurityConfig,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Import every profile from a path: a Remmina directory, a `.remmina` file, or
/// a Microsoft `.rdp` file. (Python JSON backups load via `migrate python`.)
///
/// # Errors
///
/// Returns a message when the path cannot be read or its format is unsupported.
pub fn import_path(path: &Path) -> Result<Vec<Profile>, String> {
    if path.is_dir() {
        return import_directory(path);
    }
    let text = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    let name = file_stem(path);
    match path.extension().and_then(std::ffi::OsStr::to_str) {
        Some("remmina") => Ok(vec![import_remmina(&text, &name)?]),
        Some("rdp") => Ok(vec![import_rdp(&text, &name)?]),
        Some(other) => Err(format!(
            "cannot import a .{other} file; use .remmina, .rdp, a directory, or `migrate python`"
        )),
        None => Err("cannot determine the import format from the path".into()),
    }
}

fn import_directory(path: &Path) -> Result<Vec<Profile>, String> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(path)
        .map_err(|error| error.to_string())?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(std::ffi::OsStr::to_str) == Some("remmina"))
        .collect();
    files.sort();
    if files.is_empty() {
        return Err("the directory contains no .remmina profiles".into());
    }
    let mut profiles = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(&file).map_err(|error| error.to_string())?;
        // A non-RDP Remmina profile is skipped, not fatal to the batch.
        if let Ok(profile) = import_remmina(&text, &file_stem(&file)) {
            profiles.push(profile);
        }
    }
    Ok(profiles)
}

/// Import a single Remmina `.remmina` profile.
///
/// # Errors
///
/// Returns a message for a non-RDP profile, a missing server, or an unparseable host.
pub fn import_remmina(text: &str, fallback_name: &str) -> Result<Profile, String> {
    let ini = parse_ini(text);
    let protocol = ini.get("protocol").map_or_else(
        || "RDP".to_string(),
        |value| value.trim().to_ascii_uppercase(),
    );
    if protocol != "RDP" {
        return Err("only Remmina RDP profiles can be imported".into());
    }
    let host = ini
        .get("server")
        .map(|value| value.trim().to_string())
        .unwrap_or_default();
    if host.is_empty() {
        return Err("Remmina profile has no server".into());
    }
    let (user, domain) = user_and_domain(get(&ini, "username"), get(&ini, "domain"));
    let scale = ini
        .get("scale")
        .map_or_else(|| "0".to_string(), |value| value.trim().to_string());
    let multimon = flag(&ini, "multimon") || flag(&ini, "force_multimon");
    let span = flag(&ini, "span");
    let sound = ini
        .get("sound")
        .map(|value| value.trim().to_ascii_lowercase())
        .unwrap_or_default();
    let microphone = !matches!(
        ini.get("microphone")
            .map(|value| value.trim().to_ascii_lowercase())
            .as_deref(),
        None | Some("" | "0" | "off")
    );
    Imported {
        name: ini
            .get("name")
            .filter(|name| !name.trim().is_empty())
            .map_or_else(|| fallback_name.to_string(), Clone::clone),
        host,
        user,
        domain,
        fullscreen: remmina_fullscreen(&ini),
        clipboard: !flag(&ini, "disableclipboard"),
        audio: sound.starts_with("local"),
        microphone,
        resolution: positive_resolution(
            get(&ini, "resolution_width"),
            get(&ini, "resolution_height"),
        ),
        dynamic_resolution: scale == "2" && !multimon && !span,
        multimon,
        span_monitors: span,
        smart_sizing: scale == "1",
        color_depth: remmina_color_depth(get(&ini, "colordepth")),
        admin_session: flag(&ini, "console"),
        certificate_policy: if flag(&ini, "cert_ignore") {
            CertificatePolicy::Ignore
        } else {
            CertificatePolicy::Tofu
        },
        network_profile: remmina_network(get(&ini, "network")),
        gateway: ini
            .get("gateway_server")
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
    }
    .into_profile()
}

/// Import a single Microsoft `.rdp` file.
///
/// # Errors
///
/// Returns a message when the file has no `full address` or an unparseable host.
pub fn import_rdp(text: &str, fallback_name: &str) -> Result<Profile, String> {
    let mut values = BTreeMap::new();
    for line in text.lines() {
        let parts: Vec<&str> = line.splitn(3, ':').collect();
        if parts.len() == 3 {
            values.insert(
                parts[0].trim().to_ascii_lowercase(),
                parts[2].trim().to_string(),
            );
        }
    }
    let host = values
        .get("full address")
        .map(|value| value.trim().to_string())
        .unwrap_or_default();
    if host.is_empty() {
        return Err("RDP file has no full address".into());
    }
    let (user, domain) = user_and_domain(get(&values, "username"), get(&values, "domain"));
    Imported {
        name: fallback_name.to_string(),
        host,
        user,
        domain,
        fullscreen: get(&values, "screen mode id") != "1",
        clipboard: get(&values, "redirectclipboard") != "0",
        audio: get(&values, "audiomode") == "0",
        microphone: false,
        resolution: positive_resolution(
            get(&values, "desktopwidth"),
            get(&values, "desktopheight"),
        ),
        dynamic_resolution: false,
        multimon: get(&values, "use multimon") == "1",
        span_monitors: false,
        smart_sizing: false,
        color_depth: None,
        admin_session: false,
        certificate_policy: CertificatePolicy::Tofu,
        network_profile: NetworkProfile::Auto,
        gateway: None,
    }
    .into_profile()
}

/// Render a profile as a conventional Microsoft `.rdp` file, never exporting a
/// password. Mirrors the Python exporter.
#[must_use]
pub fn export_rdp(profile: &Profile) -> String {
    let username = if !profile.identity.domain.is_empty() && !profile.identity.username.is_empty() {
        format!("{}\\{}", profile.identity.domain, profile.identity.username)
    } else {
        profile.identity.username.clone()
    };
    let mut lines = vec![
        format!(
            "screen mode id:i:{}",
            u8::from(profile.display.fullscreen) + 1
        ),
        format!("full address:s:{}", profile.endpoint),
        format!("username:s:{username}"),
        format!("domain:s:{}", profile.identity.domain),
        format!(
            "redirectclipboard:i:{}",
            i32::from(profile.devices.clipboard)
        ),
        format!(
            "audiomode:i:{}",
            if profile.devices.audio_playback { 0 } else { 2 }
        ),
        format!("use multimon:i:{}", i32::from(profile.display.multimon)),
    ];
    if let Some((width, height)) = profile.display.resolution {
        lines.push(format!("desktopwidth:i:{width}"));
        lines.push(format!("desktopheight:i:{height}"));
    }
    format!("{}\r\n", lines.join("\r\n"))
}

#[allow(clippy::struct_excessive_bools)] // Mirrors the imported profile shape.
struct Imported {
    name: String,
    host: String,
    user: String,
    domain: String,
    fullscreen: bool,
    clipboard: bool,
    audio: bool,
    microphone: bool,
    resolution: Option<(u16, u16)>,
    dynamic_resolution: bool,
    multimon: bool,
    span_monitors: bool,
    smart_sizing: bool,
    color_depth: Option<u8>,
    admin_session: bool,
    certificate_policy: CertificatePolicy,
    network_profile: NetworkProfile,
    gateway: Option<String>,
}

impl Imported {
    fn into_profile(self) -> Result<Profile, String> {
        let route = match self.gateway {
            Some(gateway) => Route::RdGateway {
                gateway: parse_endpoint(&gateway, 443)?,
                credential: None,
            },
            None => Route::Direct,
        };
        Ok(Profile {
            id: ProfileId::generate(),
            name: self.name,
            endpoint: parse_endpoint(&self.host, 3389)?,
            identity: IdentityConfig {
                username: self.user,
                domain: self.domain,
            },
            route,
            display: DisplayConfig {
                renderer: Renderer::WaylandSdl,
                fullscreen: self.fullscreen,
                resolution: self.resolution,
                dynamic_resolution: self.dynamic_resolution,
                multimon: self.multimon,
                span_monitors: self.span_monitors,
                smart_sizing: self.smart_sizing,
                scale_percent: None,
                color_depth: self.color_depth,
                graphics: crate::model::GraphicsMode::Auto,
            },
            devices: DeviceConfig {
                clipboard: self.clipboard,
                audio_playback: self.audio,
                microphone: self.microphone,
                shared_folders: Vec::new(),
                printers: false,
            },
            security: SecurityConfig {
                certificate_policy: self.certificate_policy,
                admin_session: self.admin_session,
                network_profile: self.network_profile,
                advanced: AdvancedOverrides {
                    freerdp_args: Vec::new(),
                },
            },
            credential: None,
        })
    }
}

fn parse_ini(text: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('[') || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            map.insert(key.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    map
}

fn get<'a>(map: &'a BTreeMap<String, String>, key: &str) -> &'a str {
    map.get(key).map_or("", String::as_str)
}

fn flag(map: &BTreeMap<String, String>, key: &str) -> bool {
    matches!(
        map.get(key)
            .map(|value| value.trim().to_ascii_lowercase())
            .as_deref(),
        Some("1" | "true" | "yes" | "on")
    )
}

fn user_and_domain(username: &str, domain: &str) -> (String, String) {
    if domain.is_empty()
        && let Some((parsed_domain, parsed_user)) = username.split_once('\\')
    {
        return (parsed_user.to_string(), parsed_domain.to_string());
    }
    (username.to_string(), domain.to_string())
}

fn positive_resolution(width: &str, height: &str) -> Option<(u16, u16)> {
    let width: u16 = width.trim().parse().ok()?;
    let height: u16 = height.trim().parse().ok()?;
    (width > 0 && height > 0).then_some((width, height))
}

fn remmina_fullscreen(ini: &BTreeMap<String, String>) -> bool {
    let fullscreen = get(ini, "fullscreen");
    if !fullscreen.is_empty() {
        return matches!(
            fullscreen.to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        );
    }
    let viewmode = get(ini, "viewmode");
    viewmode.is_empty() || viewmode == "3" || viewmode == "4"
}

fn remmina_network(value: &str) -> NetworkProfile {
    let value = value.trim().to_ascii_lowercase();
    let mapped = match value.as_str() {
        "none" | "autodetect" => "auto",
        "broadband" => "broadband-high",
        other => other,
    };
    match mapped {
        "modem" => NetworkProfile::Modem,
        "broadband-low" => NetworkProfile::BroadbandLow,
        "broadband-high" => NetworkProfile::BroadbandHigh,
        "wan" => NetworkProfile::Wan,
        "lan" => NetworkProfile::Lan,
        _ => NetworkProfile::Auto,
    }
}

fn remmina_color_depth(value: &str) -> Option<u8> {
    let depth: u8 = value.trim().parse().ok()?;
    matches!(depth, 8 | 15 | 16 | 24 | 32).then_some(depth)
}

fn parse_endpoint(host: &str, default_port: u16) -> Result<Endpoint, String> {
    let candidate = if host.contains(':') {
        host.to_string()
    } else {
        format!("{host}:{default_port}")
    };
    candidate
        .parse::<Endpoint>()
        .map_err(|error| format!("invalid host '{host}': {error:?}"))
}

fn file_stem(path: &Path) -> String {
    path.file_stem()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or("imported")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::{export_rdp, import_path, import_rdp, import_remmina};
    use tempfile::TempDir;

    #[test]
    fn imports_a_remmina_profile_with_expected_mappings() {
        let text = "[remmina]\nname=Office\nprotocol=RDP\nserver=10.0.0.5\nusername=CORP\\alice\nresolution_width=1920\nresolution_height=1080\nsound=local\ndisableclipboard=0\n";
        let profile = import_remmina(text, "fallback").unwrap();
        assert_eq!(profile.name, "Office");
        assert_eq!(profile.endpoint.to_string(), "10.0.0.5:3389");
        assert_eq!(profile.identity.username, "alice");
        assert_eq!(profile.identity.domain, "CORP");
        assert_eq!(profile.display.resolution, Some((1920, 1080)));
        assert!(profile.devices.audio_playback);
        assert!(profile.devices.clipboard);
    }

    #[test]
    fn remmina_zero_resolution_is_not_a_custom_resolution() {
        let text =
            "[remmina]\nprotocol=RDP\nserver=host\nresolution_width=0\nresolution_height=0\n";
        assert_eq!(
            import_remmina(text, "fallback").unwrap().display.resolution,
            None
        );
    }

    #[test]
    fn rejects_a_non_rdp_remmina_profile() {
        let text = "[remmina]\nprotocol=VNC\nserver=host\n";
        assert!(import_remmina(text, "fallback").is_err());
    }

    #[test]
    fn imports_an_rdp_file() {
        let text = "screen mode id:i:2\r\nfull address:s:10.0.0.9:3389\r\nusername:s:bob\r\naudiomode:i:0\r\n";
        let profile = import_rdp(text, "fallback").unwrap();
        assert_eq!(profile.endpoint.to_string(), "10.0.0.9:3389");
        assert_eq!(profile.identity.username, "bob");
        assert!(profile.devices.audio_playback);
        assert!(profile.display.fullscreen);
    }

    #[test]
    fn imports_a_directory_sorted_and_skips_non_rdp() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("b.remmina"),
            "[remmina]\nname=Beta\nprotocol=RDP\nserver=10.0.0.2\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("a.remmina"),
            "[remmina]\nname=Alpha\nprotocol=RDP\nserver=10.0.0.1\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("c.remmina"),
            "[remmina]\nprotocol=VNC\nserver=10.0.0.3\n",
        )
        .unwrap();

        let profiles = import_path(dir.path()).unwrap();
        assert_eq!(profiles.len(), 2);
        assert_eq!(profiles[0].name, "Alpha");
        assert_eq!(profiles[1].name, "Beta");
    }

    #[test]
    fn exports_a_profile_to_rdp_without_a_password() {
        let text = "[remmina]\nname=Office\nprotocol=RDP\nserver=10.0.0.5\nusername=CORP\\alice\nresolution_width=1920\nresolution_height=1080\n";
        let profile = import_remmina(text, "fallback").unwrap();
        let rdp = export_rdp(&profile);
        assert!(rdp.contains("full address:s:10.0.0.5:3389"));
        assert!(rdp.contains("username:s:CORP\\alice"));
        assert!(rdp.contains("desktopwidth:i:1920"));
        assert!(!rdp.to_lowercase().contains("password"));
        assert!(rdp.ends_with("\r\n"));
    }
}
