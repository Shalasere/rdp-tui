use rdp_tui::credentials::{CredentialError, CredentialStore};
use rdp_tui::model::{
    ConnectionPlan, CredentialRef, DeviceConfig, DisplayConfig, FreeRdpClient, IdentityConfig,
    PlannedRoute, ProfileId, Renderer, ResolvedCredentials, SecurityConfig, SessionId,
};
use rdp_tui::session::record::read;
use rdp_tui::session::supervisor::supervise;
use secrecy::SecretString;
use semver::Version;
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;
use tempfile::TempDir;

struct NoCredentials;

impl CredentialStore for NoCredentials {
    fn retrieve(&self, _reference: CredentialRef) -> Result<SecretString, CredentialError> {
        panic!("a passwordless plan must not retrieve any credential");
    }
}

#[test]
fn supervise_runs_a_direct_session_and_clears_its_record() {
    let temporary = TempDir::new().unwrap();
    let executable = temporary.path().join("fake-freerdp");
    std::fs::write(&executable, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();

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
            executable,
            renderer: Renderer::X11,
            version: Version::new(3, 30, 0),
        },
    };
    let session = "550e8400-e29b-41d4-a716-446655440000"
        .parse::<SessionId>()
        .unwrap();
    let profile = "111e8400-e29b-41d4-a716-446655440000"
        .parse::<ProfileId>()
        .unwrap();
    let records = TempDir::new().unwrap();

    let result = supervise(
        &plan,
        profile,
        session,
        &NoCredentials,
        Path::new("/usr/bin/rdp-tui"),
        records.path(),
        Duration::from_secs(2),
    )
    .unwrap();

    assert_eq!(result.exit_code, Some(0));
    assert!(result.failure.is_none());
    // The record is cleared once the session ends.
    assert!(read(records.path(), session).unwrap().is_none());
    drop(listener);
}
