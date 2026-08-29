use crate::model::{CertificatePolicy, NetworkProfile, PlannedRoute, PreparedConnection};
use std::ffi::OsString;
use std::path::PathBuf;
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
    if plan.display.fullscreen {
        args.push("/f".into());
    }
    if plan.devices.clipboard {
        args.push("+clipboard".into());
    }
    if plan.devices.audio_playback {
        args.push("/sound".into());
    }
    if plan.devices.microphone {
        args.push("/microphone".into());
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
    if let PlannedRoute::RdGateway { gateway } = &plan.route {
        args.push(format!("/gateway:g:{gateway}").into());
    }
    if let NetworkProfile::Auto = plan.security.network_profile {
    } else {
        args.push(format!("/network:{}", network(plan.security.network_profile)).into());
    }
    if let Some(depth) = plan.display.color_depth {
        args.push(format!("/bpp:{depth}").into());
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
