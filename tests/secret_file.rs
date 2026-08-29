use rdp_tui::credentials::CredentialStore;
use rdp_tui::secret::file::EncryptedFileStore;
use secrecy::{ExposeSecret, SecretString};
use tempfile::TempDir;

#[test]
fn encrypted_file_roundtrip_and_delete() {
    let temporary = TempDir::new().unwrap();
    let store = EncryptedFileStore::new(temporary.path());
    let reference = store.store(&SecretString::from("correct horse")).unwrap();
    assert_eq!(
        store.retrieve(reference).unwrap().expose_secret(),
        "correct horse"
    );
    assert!(
        !std::fs::read_to_string(temporary.path().join("credentials.json"))
            .unwrap()
            .contains("correct horse")
    );
    store.delete(reference).unwrap();
    assert!(store.retrieve(reference).is_err());
}
