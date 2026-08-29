use super::{AdvancedOverrides, Profile, Route};
use std::fmt;

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ProfileValidationIssue {
    pub path: String,
    pub message: String,
}

impl ProfileValidationIssue {
    fn new(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for ProfileValidationIssue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.path, self.message)
    }
}

impl Profile {
    #[must_use]
    pub fn validate(&self) -> Vec<ProfileValidationIssue> {
        let mut issues = Vec::new();
        if self.name.trim().is_empty() {
            issues.push(ProfileValidationIssue::new("name", "nickname is required"));
        }
        validate_route(&self.route, &mut issues);
        validate_display(self, &mut issues);
        validate_devices(self, &mut issues);
        validate_advanced(&self.security.advanced, &mut issues);
        issues
    }
}

fn validate_route(route: &Route, issues: &mut Vec<ProfileValidationIssue>) {
    match route {
        Route::Direct => {}
        Route::RdGateway { gateway, .. } => {
            if gateway.port == 0 {
                issues.push(ProfileValidationIssue::new(
                    "route.gateway.port",
                    "gateway port must be nonzero",
                ));
            }
        }
        Route::SshTunnel { jump_host } => {
            if jump_host.trim().is_empty() {
                issues.push(ProfileValidationIssue::new(
                    "route.jump_host",
                    "SSH jump host is required",
                ));
            } else if jump_host.chars().any(char::is_whitespace) || jump_host.starts_with('-') {
                issues.push(ProfileValidationIssue::new(
                    "route.jump_host",
                    "SSH jump host must be one config host/alias and cannot start with '-'",
                ));
            }
        }
    }
}

fn validate_display(profile: &Profile, issues: &mut Vec<ProfileValidationIssue>) {
    let display = &profile.display;
    if let Some((width, height)) = display.resolution
        && (!(200..=16_384).contains(&width) || !(200..=16_384).contains(&height))
    {
        issues.push(ProfileValidationIssue::new(
            "display.resolution",
            "width and height must each be between 200 and 16384",
        ));
    }
    if display.dynamic_resolution && display.multimon {
        issues.push(ProfileValidationIssue::new(
            "display.dynamic_resolution",
            "dynamic resolution cannot be combined with multi-monitor",
        ));
    }
    if display.dynamic_resolution && display.span_monitors {
        issues.push(ProfileValidationIssue::new(
            "display.dynamic_resolution",
            "dynamic resolution cannot be combined with monitor spanning",
        ));
    }
    if display.dynamic_resolution && display.smart_sizing {
        issues.push(ProfileValidationIssue::new(
            "display.dynamic_resolution",
            "dynamic resolution cannot be combined with smart sizing",
        ));
    }
    if display.multimon && display.span_monitors {
        issues.push(ProfileValidationIssue::new(
            "display.multimon",
            "multi-monitor and monitor spanning are mutually exclusive",
        ));
    }
    if let Some(scale) = display.scale_percent
        && ![100, 140, 180].contains(&scale)
    {
        issues.push(ProfileValidationIssue::new(
            "display.scale_percent",
            "scale must be 100, 140, or 180 percent",
        ));
    }
    if let Some(depth) = display.color_depth
        && ![8, 15, 16, 24, 32].contains(&depth)
    {
        issues.push(ProfileValidationIssue::new(
            "display.color_depth",
            "color depth must be 8, 15, 16, 24, or 32 bits",
        ));
    }
}

fn validate_devices(profile: &Profile, issues: &mut Vec<ProfileValidationIssue>) {
    for (index, folder) in profile.devices.shared_folders.iter().enumerate() {
        if !folder.is_absolute() {
            issues.push(ProfileValidationIssue::new(
                format!("devices.shared_folders[{index}]"),
                "shared folder must be an absolute path",
            ));
        }
        if folder.to_string_lossy().contains(',') {
            issues.push(ProfileValidationIssue::new(
                format!("devices.shared_folders[{index}]"),
                "shared folder cannot contain a comma",
            ));
        }
    }
}

fn validate_advanced(overrides: &AdvancedOverrides, issues: &mut Vec<ProfileValidationIssue>) {
    for (index, argument) in overrides.freerdp_args.iter().enumerate() {
        let normalized = argument.to_ascii_lowercase();
        let path = format!("security.advanced.freerdp_args[{index}]");
        if argument.is_empty()
            || argument.contains(['\0', '\n', '\r'])
            || !argument.starts_with(['/', '+', '-'])
        {
            issues.push(ProfileValidationIssue::new(
                path,
                "advanced override must be exactly one non-positional FreeRDP argument",
            ));
        } else if is_managed_freerdp_switch(&normalized) {
            issues.push(ProfileValidationIssue::new(
                path,
                "target, identity, credential, certificate, auth-only, and remote-shell switches are managed fields",
            ));
        }
    }
}

fn is_managed_freerdp_switch(argument: &str) -> bool {
    if matches!(argument, "+auth-only" | "-auth-only") {
        return true;
    }
    let Some(argument) = argument.strip_prefix('/') else {
        return false;
    };
    let name = argument
        .split_once([':', '='])
        .map_or(argument, |(name, _)| name);
    matches!(
        name,
        "v" | "server"
            | "port"
            | "server-name"
            | "u"
            | "username"
            | "d"
            | "domain"
            | "p"
            | "password"
            | "pth"
            | "g"
            | "gateway"
            | "gu"
            | "gateway-username"
            | "gd"
            | "gateway-domain"
            | "gp"
            | "gateway-password"
            | "gateway-usage-method"
            | "auth-only"
            | "from-stdin"
            | "args-from"
            | "shell"
            | "shell-dir"
            | "app"
            | "app-cmd"
            | "load-balance-info"
            | "assistance"
            | "endpointfedauth"
            | "proxy"
            | "cert"
            | "smartcard-logon"
    )
}
