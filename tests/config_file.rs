use rdp_tui::config::{ConfigDocument, ConfigStore, ProfilesDocument, StoreError};
use tempfile::TempDir;

#[test]
fn missing_documents_have_safe_defaults_and_roundtrip_durably() {
    let temporary = TempDir::new().expect("temporary config directory");
    let store = ConfigStore::new(temporary.path());

    assert_eq!(
        store.load_config().expect("default config"),
        ConfigDocument::default()
    );
    assert_eq!(
        store.load_profiles().expect("default profiles"),
        ProfilesDocument::default()
    );

    store
        .save_config(&ConfigDocument::default())
        .expect("save config");
    store
        .update_profiles(|profiles| {
            profiles.version = 1;
            Ok(())
        })
        .expect("locked profile update");

    assert_eq!(
        store.load_config().expect("read config"),
        ConfigDocument::default()
    );
    assert_eq!(
        store.load_profiles().expect("read profiles"),
        ProfilesDocument::default()
    );
    assert!(store.config_path().is_file());
    assert!(store.profiles_path().is_file());
}

#[test]
fn invalid_document_is_preserved_instead_of_being_replaced() {
    let temporary = TempDir::new().expect("temporary config directory");
    let store = ConfigStore::new(temporary.path());
    std::fs::write(store.profiles_path(), "version = 999\nprofiles = []\n")
        .expect("write invalid document");

    assert!(matches!(
        store.load_profiles(),
        Err(StoreError::Schema { .. })
    ));
    assert_eq!(
        std::fs::read_to_string(store.profiles_path()).expect("read preserved invalid document"),
        "version = 999\nprofiles = []\n"
    );
}
