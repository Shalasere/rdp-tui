use crate::model::{
    CertificatePolicy, GraphicsMode, NetworkProfile, PlannedRoute, PreparedConnection, Renderer,
};
use std::ffi::OsString;
use std::path::PathBuf;

const STABLE_MICROPHONE_REDIRECTION: &str = "/microphone:sys:pulse,format:1,rate:48000,channel:1";

/// Build `FreeRDP` argv only; credentials are deliberately not rendered into argv.
#[must_use]
pub fn build_command(
    prepared: &PreparedConnection,
) -> (PathBuf, Vec<OsString>, Vec<(OsString, OsString)>) {
    let plan = &prepared.plan;
    let mut args = vec![format!("/v:{}", prepared.effective_endpoint).into()];
    if !plan.identity.username.is_empty() {
        args.push(format!("/u:{}", plan.identity.username).into());
    }
    args.push(format!("/d:{}", plan.identity.domain).into());
    if plan.identity.domain.is_empty() {
        args.push("/auth-pkg-list:none,ntlm".into());
    }
    // Only X11 uses FreeRDP's /f. The SDL client reports a 64x64 monitor before
    // its Wayland surface is sized and fails pre-connect, so connect() supplies
    // an explicit /size for SDL fullscreen instead (matching the Python client).
    if plan.display.fullscreen && matches!(plan.client.renderer, Renderer::X11) {
        args.push("/f".into());
    }
    if plan.devices.clipboard {
        args.push("+clipboard".into());
    }
    if plan.devices.audio_playback {
        args.push("/sound".into());
    }
    if plan.devices.microphone {
        // Use the Pulse/PipeWire default source selected by the desktop, while
        // avoiding FreeRDP's unstable AAC input path observed during teardown.
        args.push(STABLE_MICROPHONE_REDIRECTION.into());
    }
    if plan.devices.printers {
        args.push("/printer".into());
    }
    for (index, folder) in plan.devices.shared_folders.iter().enumerate() {
        args.push(format!("/drive:rdp-tui-{},{}", index + 1, folder.display()).into());
    }
    match plan.security.certificate_policy {
        CertificatePolicy::Tofu => args.push("/cert:tofu".into()),
        CertificatePolicy::Ignore => args.push("/cert:ignore".into()),
        CertificatePolicy::Deny => args.push("/cert:deny".into()),
        CertificatePolicy::System => {}
    }
    if let Some((w, h)) = plan.display.resolution {
        args.push(format!("/size:{w}x{h}").into());
    }
    if plan.security.admin_session {
        args.push("/admin".into());
    }
    if plan.display.span_monitors {
        args.push("/span".into());
    } else if plan.display.multimon {
        args.push("/multimon".into());
    }
    if plan.display.smart_sizing {
        args.push("/smart-sizing".into());
    }
    if let Some(scale) = plan.display.scale_percent {
        args.push(format!("/scale:{scale}").into());
    }
    if plan.display.dynamic_resolution {
        args.push("+dynamic-resolution".into());
    }
    append_gateway_args(plan, &mut args);
    if let NetworkProfile::Auto = plan.security.network_profile {
    } else {
        args.push(format!("/network:{}", network(plan.security.network_profile)).into());
    }
    if let Some(depth) = plan.display.color_depth {
        args.push(format!("/bpp:{depth}").into());
    }
    match plan.display.graphics {
        GraphicsMode::Auto => {}
        GraphicsMode::Rfx => args.push("+rfx".into()),
        GraphicsMode::Avc420 => args.push("/gfx:AVC420".into()),
        GraphicsMode::Avc444 => args.push("/gfx:AVC444".into()),
    }
    if matches!(plan.client.renderer, Renderer::WaylandSdl) {
        // Leave compositor shortcuts and touchpad gestures with Hyprland.
        args.push("-grab-keyboard".into());
        args.push("-grab-mouse".into());
    }
    args.extend(
        plan.security
            .advanced
            .freerdp_args
            .iter()
            .cloned()
            .map(OsString::from),
    );
    (plan.client.executable.clone(), args, Vec::new())
}

fn append_gateway_args(plan: &crate::model::ConnectionPlan, args: &mut Vec<OsString>) {
    let PlannedRoute::RdGateway {
        gateway,
        username,
        domain,
    } = &plan.route
    else {
        return;
    };
    {
        if plan.client.version.major >= 3 {
            let mut gateway_argument = format!("/gateway:g:{gateway}");
            if !username.is_empty() {
                gateway_argument.push_str(",u:");
                gateway_argument.push_str(username);
            }
            if !domain.is_empty() {
                gateway_argument.push_str(",d:");
                gateway_argument.push_str(domain);
            }
            args.push(gateway_argument.into());
        } else {
            args.push(format!("/g:{gateway}").into());
            if !username.is_empty() {
                args.push(format!("/gu:{username}").into());
            }
            if !domain.is_empty() {
                args.push(format!("/gd:{domain}").into());
            }
        }
    }
}
const fn network(value: NetworkProfile) -> &'static str {
    match value {
        NetworkProfile::Auto => "auto",
        NetworkProfile::Modem => "modem",
        NetworkProfile::BroadbandLow => "broadband-low",
        NetworkProfile::BroadbandHigh => "broadband-high",
        NetworkProfile::Wan => "wan",
        NetworkProfile::Lan => "lan",
    }
}
