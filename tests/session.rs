use rdp_tui::model::{
    ConnectionPlan, DeviceConfig, DisplayConfig, FreeRdpClient, IdentityConfig, PlannedRoute,
    Renderer, ResolvedCredentials, SecurityConfig, SessionId,
};
use rdp_tui::preflight::prepare;
use rdp_tui::session::run;
use semver::Version;
use std::os::unix::fs::PermissionsExt;
use tempfile::TempDir;
#[test]
fn session_reports_successful_owned_client_exit() {
    let temporary = TempDir::new().unwrap();
    let executable = temporary.path().join("fake-freerdp");
    std::fs::write(&executable, "#!/bin/sh\nexit 0\n").unwrap();
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
    let result = run(
        prepare(&plan).unwrap(),
        "550e8400-e29b-41d4-a716-446655440000"
            .parse::<SessionId>()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(result.exit_code, Some(0));
    assert!(result.failure.is_none());
}
