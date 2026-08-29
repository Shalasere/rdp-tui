use rdp_tui::freerdp::process::launch;
use rdp_tui::model::{
    ConnectionPlan, DeviceConfig, DisplayConfig, FreeRdpClient, IdentityConfig, PlannedRoute,
    Renderer, ResolvedCredentials, SecurityConfig, SessionId,
};
use rdp_tui::preflight::prepare;
use rdp_tui::runtime::process::LaunchMode;
use semver::Version;
use std::os::unix::fs::PermissionsExt;
use tempfile::TempDir;

#[test]
fn launch_uses_runtime_identity_ownership() {
    let temporary = TempDir::new().unwrap();
    let executable = temporary.path().join("fake-freerdp");
    std::fs::write(&executable, "#!/bin/sh\nsleep 5\n").unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    let plan = ConnectionPlan {
        target: "anima:3389".parse().unwrap(),
        route: PlannedRoute::Direct,
        identity: IdentityConfig::default(),
        display: DisplayConfig::default(),
        devices: DeviceConfig::default(),
        security: SecurityConfig::default(),
        credentials: ResolvedCredentials::default(),
        client: FreeRdpClient {
            executable,
            renderer: Renderer::X11,
            version: Version::new(3, 30, 0),
        },
    };
    let prepared = prepare(&plan).unwrap();
    let mut child = launch(
        &prepared,
        "550e8400-e29b-41d4-a716-446655440000"
            .parse::<SessionId>()
            .unwrap(),
        None,
        LaunchMode::OneShot,
    )
    .unwrap();
    assert!(child.terminate_if_owned().unwrap().is_some());
}
