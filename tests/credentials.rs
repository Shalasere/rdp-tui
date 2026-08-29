use rdp_tui::credentials::{CredentialError, CredentialStore, acquire};
use rdp_tui::model::{CredentialBackend, CredentialKey, CredentialRef, ResolvedCredentials};
use secrecy::{ExposeSecret, SecretString};

struct TestStore;
impl CredentialStore for TestStore {
    fn retrieve(&self, reference: CredentialRef) -> Result<SecretString, CredentialError> {
        if reference.key == CredentialKey::from_bytes([0; 32]) {
            Err(CredentialError::Missing)
        } else {
            Ok(SecretString::from("not-in-argv"))
        }
    }
}

#[test]
fn acquisition_returns_nonserializable_short_lived_secrets() {
    let reference = CredentialRef {
        backend: CredentialBackend::EncryptedFile,
        key: CredentialKey::from_bytes([1; 32]),
    };
    let lease = acquire(
        &TestStore,
        ResolvedCredentials {
            main: Some(reference),
            gateway: Some(reference),
        },
    )
    .unwrap();
    assert_eq!(lease.main.as_ref().unwrap().expose_secret(), "not-in-argv");
    assert_eq!(
        lease.gateway.as_ref().unwrap().expose_secret(),
        "not-in-argv"
    );
    assert_eq!(
        format!("{lease:?}"),
        "CredentialLease { main: Some(\"<redacted>\"), gateway: Some(\"<redacted>\") }"
    );
}
