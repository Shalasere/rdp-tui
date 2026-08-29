use rdp_tui::credentials::CredentialStore;
use rdp_tui::secret::file::EncryptedFileStore;
use secrecy::{ExposeSecret, SecretString};
use std::os::unix::fs::PermissionsExt as _;
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

#[test]
fn encrypted_file_storage_is_owner_only() {
    let temporary = TempDir::new().unwrap();
    let store = EncryptedFileStore::new(temporary.path());
    store.store(&SecretString::from("correct horse")).unwrap();

    for file in [".credential-key", "credentials.json", ".credentials.lock"] {
        let mode = std::fs::metadata(temporary.path().join(file))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "unexpected mode for {file}");
    }
    let mode = std::fs::metadata(temporary.path())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o700);
}
