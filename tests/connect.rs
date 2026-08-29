use rdp_tui::model::{
    ConnectionPlan, DeviceConfig, DisplayConfig, FreeRdpClient, IdentityConfig, PlannedRoute,
    ProfileId, Renderer, ResolvedCredentials, SecurityConfig, SessionId,
};
use rdp_tui::session::launcher::spawn_supervisor;
use semver::Version;
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::{Duration, Instant};
use tempfile::TempDir;

/// Drive the whole connect chain through the real binary: the launcher spawns
/// `rdp-tui __supervise`, hands it a plan over the inherited pipe, and the
/// detached supervisor preflights a live listener and launches a fake `FreeRDP`.
#[test]
fn spawn_supervisor_launches_a_detached_session_via_the_real_binary() {
    let dir = TempDir::new().unwrap();
    let marker = dir.path().join("launched");
    let freerdp = dir.path().join("fake-freerdp");
    std::fs::write(
        &freerdp,
        format!("#!/bin/sh\ntouch '{}'\nexit 0\n", marker.display()),
    )
    .unwrap();
    std::fs::set_permissions(&freerdp, std::fs::Permissions::from_mode(0o700)).unwrap();

    // A live loopback listener makes the direct preflight reachable.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let target = format!("127.0.0.1:{}", listener.local_addr().unwrap().port());

    let plan = ConnectionPlan {
        target: target.parse().unwrap(),
        route: PlannedRoute::Direct,
        identity: IdentityConfig::default(),
        display: DisplayConfig::default(),
        devices: DeviceConfig::default(),
        security: SecurityConfig::default(),
        credentials: ResolvedCredentials::default(),
        client: FreeRdpClient {
            executable: freerdp,
            renderer: Renderer::X11,
            version: Version::new(3, 30, 0),
        },
    };

    let executable = Path::new(env!("CARGO_BIN_EXE_rdp-tui"));
    spawn_supervisor(
        &plan,
        ProfileId::generate(),
        SessionId::generate(),
        executable,
    )
    .unwrap();

    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && !marker.exists() {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        marker.exists(),
        "the detached supervisor should have launched the fake FreeRDP"
    );
    drop(listener);
}
