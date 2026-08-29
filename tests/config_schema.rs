use rdp_tui::config::{
    ConfigDocument, ProfilesDocument, StoreError, parse_config_document, parse_profiles_document,
};
use rdp_tui::model::{
    DeviceConfig, DisplayConfig, IdentityConfig, Profile, ProfileId, Route, SecurityConfig,
};

fn profile() -> Profile {
    Profile {
        id: "550e8400-e29b-41d4-a716-446655440000"
            .parse::<ProfileId>()
            .unwrap(),
        name: "Anima".into(),
        endpoint: "10.0.0.111".parse().unwrap(),
        identity: IdentityConfig::default(),
        route: Route::Direct,
        display: DisplayConfig::default(),
        devices: DeviceConfig::default(),
        security: SecurityConfig::default(),
        credential: None,
    }
}

#[test]
fn config_and_profiles_toml_roundtrip() {
    let config = ConfigDocument::default();
    let config_toml = toml::to_string_pretty(&config).unwrap();
    assert_eq!(parse_config_document(&config_toml).unwrap(), config);

    let profiles = ProfilesDocument {
        version: 1,
        profiles: vec![profile()],
    };
    let profiles_toml = toml::to_string_pretty(&profiles).unwrap();
    assert_eq!(parse_profiles_document(&profiles_toml).unwrap(), profiles);
}

#[test]
fn unsupported_schema_versions_are_rejected() {
    let error = parse_profiles_document("version = 2\nprofiles = []\n").unwrap_err();
    assert!(matches!(
        error,
        StoreError::Schema { path, found, .. } if path == "version" && found == "2"
    ));
}

#[test]
fn unknown_secret_fields_are_rejected() {
    let document = ProfilesDocument {
        version: 1,
        profiles: vec![profile()],
    };
    let original = toml::to_string_pretty(&document).unwrap();
    let with_password = original.replacen(
        "name = \"Anima\"",
        "name = \"Anima\"\npassword = \"must-not-be-accepted\"",
        1,
    );
    assert_ne!(original, with_password);
    let error = parse_profiles_document(&with_password).unwrap_err();
    assert!(error.to_string().contains("unknown field `password`"));
}

#[test]
fn duplicate_profile_ids_are_rejected() {
    let duplicate = profile();
    let document = ProfilesDocument {
        version: 1,
        profiles: vec![profile(), duplicate],
    };
    let error = parse_profiles_document(&toml::to_string_pretty(&document).unwrap()).unwrap_err();
    assert!(matches!(
        error,
        StoreError::Schema { path, .. } if path == "profiles[1].id"
    ));
}

#[test]
fn unsafe_advanced_overrides_are_rejected() {
    for argument in [
        "/v:other-host",
        "/username:other-user",
        "/p:plaintext",
        "/gateway-password:plaintext",
        "/auth-only",
        "/shell:cmd.exe",
        "xfreerdp3",
    ] {
        let mut invalid = profile();
        invalid.security.advanced.freerdp_args = vec![argument.into()];
        let document = ProfilesDocument {
            version: 1,
            profiles: vec![invalid],
        };
        let error =
            parse_profiles_document(&toml::to_string_pretty(&document).unwrap()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("security.advanced.freerdp_args[0]"),
            "{argument} was not rejected"
        );
    }
}

#[test]
fn incompatible_display_modes_are_rejected_without_io() {
    let mut invalid = profile();
    invalid.display.dynamic_resolution = true;
    invalid.display.multimon = true;
    let issues = invalid.validate();
    assert!(
        issues
            .iter()
            .any(|issue| issue.path == "display.dynamic_resolution")
    );
}
