use rdp_tui::ProfileStore;
use rdp_tui::config::ConfigStore;
use rdp_tui::model::{
    DeviceConfig, DisplayConfig, Endpoint, IdentityConfig, Profile, ProfileId, Route,
    SecurityConfig,
};
use tempfile::TempDir;

fn profile(id: &str, name: &str) -> Profile {
    Profile {
        id: id.parse::<ProfileId>().expect("valid fixed profile ID"),
        name: name.into(),
        endpoint: "anima:3389".parse::<Endpoint>().expect("valid endpoint"),
        identity: IdentityConfig::default(),
        route: Route::default(),
        display: DisplayConfig::default(),
        devices: DeviceConfig::default(),
        security: SecurityConfig::default(),
        credential: None,
    }
}

#[test]
fn upsert_get_list_and_remove_use_the_durable_store() {
    let temporary = TempDir::new().expect("temporary config directory");
    let store = ProfileStore::new(ConfigStore::new(temporary.path()));
    let id = "550e8400-e29b-41d4-a716-446655440000".parse().unwrap();

    store
        .upsert(profile("550e8400-e29b-41d4-a716-446655440000", "Anima"))
        .unwrap();
    assert_eq!(store.list().unwrap().len(), 1);
    assert_eq!(store.get(id).unwrap().unwrap().name, "Anima");

    store
        .upsert(profile("550e8400-e29b-41d4-a716-446655440000", "Tofu"))
        .unwrap();
    assert_eq!(store.list().unwrap().len(), 1);
    assert_eq!(store.get(id).unwrap().unwrap().name, "Tofu");
    assert!(store.remove(id).unwrap());
    assert!(!store.remove(id).unwrap());
}
