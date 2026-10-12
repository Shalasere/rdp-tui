use rdp_tui::credentials::{CredentialError, CredentialStore};
use rdp_tui::model::{
    ConnectionFailure, ConnectionPlan, CredentialRef, DeviceConfig, DisplayConfig, FreeRdpClient,
    IdentityConfig, PlannedRoute, ProfileId, Renderer, ResolvedCredentials, SecurityConfig,
    SessionId,
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
    let state = TempDir::new().unwrap();

    let result = supervise(
        &plan,
        profile,
        session,
        &NoCredentials,
        Path::new("/usr/bin/rdp-tui"),
        temporary.path(),
        records.path(),
        state.path(),
        Duration::from_secs(2),
    )
    .unwrap();

    assert_eq!(result.exit_code, Some(0));
    assert!(result.failure.is_none());
    // The record is cleared once the session ends.
    assert!(read(records.path(), session).unwrap().is_none());
    // The successful session is appended to the connection history.
    let history = rdp_tui::config::ConfigStore::new(state.path())
        .load_history()
        .unwrap();
    assert_eq!(history.entries.len(), 1);
    assert_eq!(history.entries[0].profile_id, profile);
    assert!(history.entries[0].succeeded());
    drop(listener);
}

#[test]
fn supervise_interrupts_a_changed_certificate_and_reports_it() {
    let temporary = TempDir::new().unwrap();
    let executable = temporary.path().join("fake-freerdp");
    // Emit a FreeRDP changed-certificate report, then block; the monitor must
    // detect the change and terminate this child rather than wait on it.
    std::fs::write(
        &executable,
        "#!/bin/sh\necho 'The certificate does not match the previously stored one. The fingerprint is 09:7f:45:ad:7e:ad:dd:47:df:ee:3f:2b:1a:d1:ea:ec:a7:c3:14:b7:f2:38:e1:30:a5:c0:58:c6:ae:89:23:50'\nsleep 30\n",
    )
    .unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();

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
    let state = TempDir::new().unwrap();

    let result = supervise(
        &plan,
        profile,
        session,
        &NoCredentials,
        Path::new("/usr/bin/rdp-tui"),
        temporary.path(),
        records.path(),
        state.path(),
        Duration::from_secs(2),
    )
    .unwrap();

    assert!(matches!(
        result.failure,
        Some(ConnectionFailure::Certificate)
    ));
    assert!(read(records.path(), session).unwrap().is_none());
    let mismatch = rdp_tui::freerdp::certificate::read_mismatch(state.path(), profile)
        .unwrap()
        .expect("the presented fingerprint should be persisted for exact confirmation");
    assert_eq!(
        mismatch.presented_sha256,
        "097F45AD7EADDD47DFEE3F2B1AD1EAECA7C314B7F238E130A5C058C6AE892350"
    );
    let internal_logs: Vec<_> = std::fs::read_dir(state.path().join("session-logs"))
        .map(|entries| entries.filter_map(Result::ok).collect())
        .unwrap_or_default();
    assert!(internal_logs.is_empty(), "internal logs must be ephemeral");
    drop(listener);
}

#[test]
fn supervisor_distinguishes_rejected_credentials_from_an_abort_with_the_same_exit_code() {
    for (output, failure, established, ended_by_user) in [
        (
            "ERRCONNECT_LOGON_FAILURE [0x00020014]",
            Some(ConnectionFailure::Authentication),
            false,
            false,
        ),
        (
            "client aborted unexpectedly",
            Some(ConnectionFailure::ProcessFailure),
            false,
            false,
        ),
        (
            "Local framebuffer format PIXEL_FORMAT_BGRA32",
            Some(ConnectionFailure::ProcessFailure),
            true,
            false,
        ),
        (
            "Local framebuffer format PIXEL_FORMAT_BGRA32 Connection aborted by user",
            None,
            true,
            true,
        ),
    ] {
        let temporary = TempDir::new().unwrap();
        let executable = temporary.path().join("fake-freerdp");
        std::fs::write(
            &executable,
            format!("#!/bin/sh\necho '{output}' >&2\nsleep 0.05\nexit 134\n"),
        )
        .unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let plan = ConnectionPlan {
            target: listener.local_addr().unwrap().to_string().parse().unwrap(),
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
        let session = SessionId::generate();
        let profile = ProfileId::generate();
        let records = TempDir::new().unwrap();
        let state = TempDir::new().unwrap();
        let run = |session| {
            supervise(
                &plan,
                profile,
                session,
                &NoCredentials,
                Path::new("/usr/bin/rdp-tui"),
                temporary.path(),
                records.path(),
                state.path(),
                Duration::from_secs(2),
            )
        };
        let result = run(session).unwrap();
        assert_eq!(result.exit_code, Some(134));
        assert_eq!(result.failure, failure);
        assert_eq!(result.established, established);
        assert_eq!(result.ended_by_user, ended_by_user);
        let history = rdp_tui::config::ConfigStore::new(state.path())
            .load_history()
            .unwrap();
        assert_eq!(history.entries[0].session_id, Some(session));
        assert_eq!(history.entries[0].failure, failure);
        assert_eq!(history.entries[0].established, established);
        assert_eq!(history.entries[0].ended_by_user, ended_by_user);
        assert!(read(records.path(), session).unwrap().is_none());
        assert!(!records.path().join(format!("{session}.log")).exists());

        // Even a failure to spawn the client must publish a final result.
        std::fs::remove_file(&plan.client.executable).unwrap();
        let failed_session = SessionId::generate();
        assert!(run(failed_session).is_err());
        let history = rdp_tui::config::ConfigStore::new(state.path())
            .load_history()
            .unwrap();
        let failed = history
            .entries
            .iter()
            .find(|entry| entry.session_id == Some(failed_session))
            .unwrap();
        assert_eq!(failed.failure, Some(ConnectionFailure::ProcessFailure));
    }
}
