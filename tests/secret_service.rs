use rdp_tui::credentials::{CredentialError, CredentialStore};
use rdp_tui::model::{CredentialBackend, CredentialKey, CredentialRef};
use rdp_tui::secret::service::SecretServiceStore;

#[test]
fn mismatched_backend_is_rejected_before_running_secret_tool() {
    let store = SecretServiceStore::new("/definitely/not/a/secret-tool");
    let reference = CredentialRef {
        backend: CredentialBackend::EncryptedFile,
        key: CredentialKey::from_bytes([1; 32]),
    };
    assert!(matches!(
        store.retrieve(reference),
        Err(CredentialError::Unavailable(_))
    ));
}
