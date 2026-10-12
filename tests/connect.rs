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

/// A failed detached preflight must report through history, without injecting
/// raw error text into the launcher's output (or a live TUI's drawing buffer).
#[test]
fn detached_preflight_failure_does_not_leak_output_and_retains_diagnostics() {
    let dir = TempDir::new().unwrap();
    let config_root = dir.path().join("config/rdp-tui");
    let state_root = dir.path().join("state/rdp-tui");
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let frontend = bin.join("sdl-freerdp3");
    std::fs::write(&frontend, "#!/bin/sh\necho 'FreeRDP version 3.30.0'\n").unwrap();
    std::fs::set_permissions(&frontend, std::fs::Permissions::from_mode(0o700)).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = listener.local_addr().unwrap().to_string();
    drop(listener);
    let added =
        rdp_tui::cli::commands::run(&["add".into(), "Offline".into(), endpoint], &config_root)
            .unwrap();
    let profile_id = added.split_whitespace().last().unwrap();
    let diagnostics = dir.path().join("diagnostics.log");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_rdp-tui"))
        .args(["connect", profile_id])
        .env("XDG_CONFIG_HOME", dir.path().join("config"))
        .env("XDG_STATE_HOME", dir.path().join("state"))
        .env("XDG_RUNTIME_DIR", dir.path().join("runtime"))
        .env("PATH", &bin)
        .env("RDP_TUI_LOG", &diagnostics)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(
        output.stderr.is_empty(),
        "supervisor wrote onto launcher stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8(output.stdout).unwrap().lines().count(), 1);
    let store = rdp_tui::config::ConfigStore::new(&state_root);
    let deadline = Instant::now() + Duration::from_secs(5);
    let entry = loop {
        if let Some(entry) = store.load_history().unwrap().entries.pop() {
            break entry;
        }
        assert!(
            Instant::now() < deadline,
            "failed session did not publish its result"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(
        entry.failure,
        Some(rdp_tui::model::ConnectionFailure::Network)
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if std::fs::read_to_string(&diagnostics)
            .unwrap()
            .contains("supervisor failed: preflight failed:")
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "detailed diagnostic was not retained"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}
