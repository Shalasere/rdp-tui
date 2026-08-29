use rdp_tui::credentials::{CredentialError, CredentialStore};
use rdp_tui::model::{
    ConnectionPlan, CredentialBackend, CredentialKey, CredentialRef, DeviceConfig, DisplayConfig,
    FreeRdpClient, IdentityConfig, PlannedRoute, Renderer, ResolvedCredentials, SecurityConfig,
    SessionId,
};
use rdp_tui::preflight::prepare;
use rdp_tui::session::{run, run_with_credentials};
use secrecy::SecretString;
use semver::Version;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use tempfile::TempDir;

struct FixedCredentialStore;

impl CredentialStore for FixedCredentialStore {
    fn retrieve(&self, reference: CredentialRef) -> Result<SecretString, CredentialError> {
        assert_eq!(reference.backend, CredentialBackend::EncryptedFile);
        Ok(SecretString::from("never-in-environment"))
    }
}
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

#[test]
fn credential_aware_session_uses_askpass_metadata_without_secret_environment() {
    let temporary = TempDir::new().unwrap();
    let executable = temporary.path().join("fake-freerdp");
    std::fs::write(
        &executable,
        "#!/bin/sh\n[ -n \"$FREERDP_ASKPASS\" ] && [ -n \"$RDP_TUI_ASKPASS_MAIN_FD\" ] && ! env | grep -Fq never-in-environment\n",
    )
    .unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut plan = ConnectionPlan {
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
    plan.credentials.main = Some(CredentialRef {
        backend: CredentialBackend::EncryptedFile,
        key: CredentialKey::from_bytes([9; 32]),
    });
    let result = run_with_credentials(
        prepare(&plan).unwrap(),
        "550e8400-e29b-41d4-a716-446655440000"
            .parse::<SessionId>()
            .unwrap(),
        &FixedCredentialStore,
        Path::new("/usr/bin/rdp-tui"),
    )
    .unwrap();
    assert_eq!(result.exit_code, Some(0));
}
