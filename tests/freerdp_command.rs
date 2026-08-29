use rdp_tui::freerdp::command::build_command;
use rdp_tui::model::{
    CertificatePolicy, ConnectionPlan, DeviceConfig, DisplayConfig, FreeRdpClient, IdentityConfig,
    PlannedRoute, Renderer, ResolvedCredentials, SecurityConfig,
};
use rdp_tui::preflight::prepare;
use semver::Version;
use std::path::PathBuf;

#[test]
fn command_uses_prepared_endpoint_and_never_credential_references() {
    let mut plan = ConnectionPlan {
        target: "anima:3389".parse().unwrap(),
        route: PlannedRoute::Direct,
        identity: IdentityConfig::default(),
        display: DisplayConfig::default(),
        devices: DeviceConfig::default(),
        security: SecurityConfig::default(),
        credentials: ResolvedCredentials::default(),
        client: FreeRdpClient {
            executable: PathBuf::from("xfreerdp3"),
            renderer: Renderer::X11,
            version: Version::new(3, 30, 0),
        },
    };
    plan.display.fullscreen = true;
    plan.display.multimon = true;
    plan.display.resolution = Some((1920, 1080));
    plan.security.certificate_policy = CertificatePolicy::Tofu;
    let mut prepared = prepare(&plan).unwrap();
    prepared.effective_endpoint = "tofu:3390".parse().unwrap();
    let (_, args, environment) = build_command(&prepared);
    let args = args
        .iter()
        .map(|arg| arg.to_string_lossy())
        .collect::<Vec<_>>();
    assert!(args.contains(&"/v:tofu:3390".into()));
    assert!(!args.contains(&"/v:anima:3389".into()));
    assert!(args.contains(&"/cert:tofu".into()));
    assert!(args.contains(&"/multimon".into()));
    assert!(args.contains(&"/size:1920x1080".into()));
    assert!(environment.is_empty());
}
