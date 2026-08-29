//! UI-agnostic parsing, formatting, and cycling for the editable profile fields.
//!
//! Both the CLI `set` command and the TUI edit form must offer the same fields
//! (manifest `shared_logic: [tui, cli]`), and neither frontend may depend on the
//! other (the `tui`/`cli` edge is forbidden). So the one implementation lives
//! here in `model`, which both are allowed to depend on. Everything here is pure
//! and process-free — no argv, no terminal, no I/O.

use crate::model::{CertificatePolicy, Endpoint, Renderer, Route};

impl Renderer {
    /// Every renderer, in cycle order.
    pub const ALL: [Self; 2] = [Self::WaylandSdl, Self::X11];

    /// The stable token used in the CLI and on screen.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::WaylandSdl => "wayland_sdl",
            Self::X11 => "x11",
        }
    }

    /// Parse a renderer token (with a couple of friendly aliases).
    #[must_use]
    pub fn from_token(value: &str) -> Option<Self> {
        match value {
            "wayland_sdl" | "sdl" | "wayland" => Some(Self::WaylandSdl),
            "x11" | "xfreerdp" => Some(Self::X11),
            _ => None,
        }
    }

    /// The next renderer, wrapping — for a toggle/cycle control.
    #[must_use]
    pub const fn cycled(self) -> Self {
        match self {
            Self::WaylandSdl => Self::X11,
            Self::X11 => Self::WaylandSdl,
        }
    }
}

impl CertificatePolicy {
    /// Every policy, in cycle order.
    pub const ALL: [Self; 4] = [Self::Tofu, Self::System, Self::Ignore, Self::Deny];

    /// The stable token used in the CLI and on screen.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Tofu => "tofu",
            Self::System => "system",
            Self::Ignore => "ignore",
            Self::Deny => "deny",
        }
    }

    /// Parse a certificate-policy token.
    #[must_use]
    pub fn from_token(value: &str) -> Option<Self> {
        match value {
            "tofu" => Some(Self::Tofu),
            "system" => Some(Self::System),
            "ignore" => Some(Self::Ignore),
            "deny" => Some(Self::Deny),
            _ => None,
        }
    }

    /// The next policy, wrapping — for a cycle control.
    #[must_use]
    pub const fn cycled(self) -> Self {
        match self {
            Self::Tofu => Self::System,
            Self::System => Self::Ignore,
            Self::Ignore => Self::Deny,
            Self::Deny => Self::Tofu,
        }
    }
}

impl Route {
    /// A `set`-style token: `direct`, `gateway:<host>`, or `ssh:<jump-host>`.
    #[must_use]
    pub fn to_token(&self) -> String {
        match self {
            Self::Direct => "direct".to_owned(),
            Self::RdGateway { gateway, .. } => format!("gateway:{gateway}"),
            Self::SshTunnel { jump_host } => format!("ssh:{jump_host}"),
        }
    }

    /// Parse a `set`-style route token. An unqualified gateway defaults to :443.
    ///
    /// # Errors
    ///
    /// Returns a message when the token is unknown or its endpoint is invalid.
    pub fn from_token(value: &str) -> Result<Self, String> {
        if value == "direct" {
            return Ok(Self::Direct);
        }
        if let Some(host) = value.strip_prefix("gateway:") {
            let with_port = if host.contains(':') {
                host.to_owned()
            } else {
                format!("{host}:443")
            };
            let gateway = with_port
                .parse::<Endpoint>()
                .map_err(|error| format!("invalid gateway '{host}': {error}"))?;
            return Ok(Self::RdGateway {
                gateway,
                credential: None,
            });
        }
        if let Some(jump_host) = value.strip_prefix("ssh:") {
            if jump_host.is_empty() {
                return Err("ssh route needs a jump host (ssh:<host>)".to_owned());
            }
            return Ok(Self::SshTunnel {
                jump_host: jump_host.to_owned(),
            });
        }
        Err(format!(
            "unknown route '{value}' (use direct | gateway:<host> | ssh:<jump-host>)"
        ))
    }
}

/// The scale-percent choices the validator accepts, plus the monitor default.
pub const SCALE_CHOICES: [Option<u16>; 4] = [None, Some(100), Some(140), Some(180)];

/// The color-depth choices the validator accepts, plus the client default.
pub const COLOR_DEPTH_CHOICES: [Option<u8>; 6] =
    [None, Some(8), Some(15), Some(16), Some(24), Some(32)];

/// Render a boolean as the yes/no token used everywhere.
#[must_use]
pub const fn format_bool(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

/// Parse a yes/no token.
///
/// # Errors
///
/// Returns a message for anything that is not a recognized yes/no value.
pub fn parse_bool(value: &str) -> Result<bool, String> {
    match value.to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Ok(true),
        "false" | "no" | "off" | "0" => Ok(false),
        other => Err(format!("expected a yes/no value, got '{other}'")),
    }
}

/// Render a resolution as `WIDTHxHEIGHT`, or `none` for the monitor default.
#[must_use]
pub fn format_resolution(resolution: Option<(u16, u16)>) -> String {
    match resolution {
        Some((width, height)) => format!("{width}x{height}"),
        None => "none".to_owned(),
    }
}

/// Parse `WIDTHxHEIGHT`, or `none`/empty for the monitor default.
///
/// # Errors
///
/// Returns a message when the token is not a valid `WIDTHxHEIGHT` pair.
pub fn parse_resolution(value: &str) -> Result<Option<(u16, u16)>, String> {
    if value.is_empty() || value.eq_ignore_ascii_case("none") {
        return Ok(None);
    }
    let (width, height) = value
        .split_once(['x', 'X'])
        .ok_or_else(|| format!("resolution must be WIDTHxHEIGHT, got '{value}'"))?;
    let width = width
        .trim()
        .parse::<u16>()
        .map_err(|_| format!("invalid width in '{value}'"))?;
    let height = height
        .trim()
        .parse::<u16>()
        .map_err(|_| format!("invalid height in '{value}'"))?;
    Ok(Some((width, height)))
}

/// Render a scale percent as `140%`, or `auto` for the monitor default.
#[must_use]
pub fn format_scale(scale: Option<u16>) -> String {
    scale.map_or_else(|| "auto".to_owned(), |percent| format!("{percent}%"))
}

/// The next accepted scale, wrapping — for a cycle control.
#[must_use]
pub fn cycle_scale(current: Option<u16>) -> Option<u16> {
    cycle_choice(&SCALE_CHOICES, current)
}

/// Parse a scale percent, or `none`/empty for the monitor default. The store's
/// own validation still enforces the accepted set.
///
/// # Errors
///
/// Returns a message for a value that is not a number or `none`.
pub fn parse_scale(value: &str) -> Result<Option<u16>, String> {
    parse_optional(value)
}

/// Render a color depth as `24-bit`, or `auto` for the client default.
#[must_use]
pub fn format_color_depth(depth: Option<u8>) -> String {
    depth.map_or_else(|| "auto".to_owned(), |bits| format!("{bits}-bit"))
}

/// The next accepted color depth, wrapping — for a cycle control.
#[must_use]
pub fn cycle_color_depth(current: Option<u8>) -> Option<u8> {
    cycle_choice(&COLOR_DEPTH_CHOICES, current)
}

/// Parse a color depth, or `none`/empty for the client default. The store's own
/// validation still enforces the accepted set.
///
/// # Errors
///
/// Returns a message for a value that is not a number or `none`.
pub fn parse_color_depth(value: &str) -> Result<Option<u8>, String> {
    parse_optional(value)
}

fn cycle_choice<T: Copy + PartialEq>(choices: &[T], current: T) -> T {
    let index = choices
        .iter()
        .position(|choice| *choice == current)
        .unwrap_or(0);
    choices[(index + 1) % choices.len()]
}

fn parse_optional<T: std::str::FromStr>(value: &str) -> Result<Option<T>, String> {
    if value.is_empty() || value.eq_ignore_ascii_case("none") {
        return Ok(None);
    }
    value
        .parse::<T>()
        .map(Some)
        .map_err(|_| format!("expected a number or 'none', got '{value}'"))
}

#[cfg(test)]
mod tests {
    use super::{cycle_scale, format_resolution, parse_bool, parse_resolution};
    use crate::model::{Renderer, Route};

    #[test]
    fn renderer_tokens_round_trip() {
        for renderer in Renderer::ALL {
            assert_eq!(Renderer::from_token(renderer.token()), Some(renderer));
        }
        assert_eq!(Renderer::from_token("sdl"), Some(Renderer::WaylandSdl));
        assert_eq!(Renderer::from_token("nope"), None);
    }

    #[test]
    fn route_tokens_round_trip() {
        let direct = Route::from_token("direct").unwrap();
        assert_eq!(direct.to_token(), "direct");
        let ssh = Route::from_token("ssh:jump.example").unwrap();
        assert!(matches!(ssh, Route::SshTunnel { .. }));
        assert_eq!(ssh.to_token(), "ssh:jump.example");
        let gateway = Route::from_token("gateway:gw.example").unwrap();
        assert_eq!(gateway.to_token(), "gateway:gw.example:443");
        assert!(Route::from_token("ssh:").is_err());
        assert!(Route::from_token("bogus").is_err());
    }

    #[test]
    fn resolution_and_bool_parse() {
        assert_eq!(parse_resolution("1920x1080").unwrap(), Some((1920, 1080)));
        assert_eq!(parse_resolution("none").unwrap(), None);
        assert!(parse_resolution("wide").is_err());
        assert_eq!(format_resolution(Some((800, 600))), "800x600");
        assert!(parse_bool("yes").unwrap());
        assert!(!parse_bool("off").unwrap());
    }

    #[test]
    fn scale_cycles_through_the_accepted_set() {
        assert_eq!(cycle_scale(None), Some(100));
        assert_eq!(cycle_scale(Some(100)), Some(140));
        assert_eq!(cycle_scale(Some(180)), None);
    }
}
