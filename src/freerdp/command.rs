use crate::model::{CertificatePolicy, ConnectionPlan, NetworkProfile, PlannedRoute};
use std::ffi::OsString;
use std::path::PathBuf;
/// Build `FreeRDP` argv only; credentials are deliberately not rendered into argv.
#[must_use]
pub fn build_command(plan: &ConnectionPlan) -> (PathBuf, Vec<OsString>, Vec<(OsString, OsString)>) {
    let mut args = vec![format!("/v:{}", plan.target).into()];
    if !plan.identity.username.is_empty() {
        args.push(format!("/u:{}", plan.identity.username).into());
    }
    args.push(format!("/d:{}", plan.identity.domain).into());
    if plan.display.fullscreen {
        args.push("/f".into());
    }
    if plan.devices.clipboard {
        args.push("+clipboard".into());
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
    if plan.display.multimon {
        args.push("/multimon".into());
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
