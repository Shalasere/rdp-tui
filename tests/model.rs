use rdp_tui::model::{
    CertificatePolicy, ConnectionPlan, DeviceConfig, DisplayConfig, FreeRdpClient, IdentityConfig,
    PlannedRoute, Profile, ProfileId, Renderer, ResolvedCredentials, Route, SecurityConfig,
};
use semver::Version;
use std::fs;
use std::path::{Path, PathBuf};

fn profile() -> Profile {
    Profile {
        id: "550e8400-e29b-41d4-a716-446655440000"
            .parse::<ProfileId>()
            .unwrap(),
        name: "Anima".into(),
        endpoint: "10.0.0.111:3389".parse().unwrap(),
        identity: IdentityConfig {
            username: "ada".into(),
            domain: "LAB".into(),
        },
        route: Route::Direct,
        display: DisplayConfig::default(),
        devices: DeviceConfig::default(),
        security: SecurityConfig::default(),
        credential: None,
    }
}

fn client() -> FreeRdpClient {
    FreeRdpClient {
        executable: PathBuf::from("/usr/bin/sdl-freerdp3"),
        renderer: Renderer::WaylandSdl,
        version: Version::new(3, 12, 0),
    }
}

#[test]
fn profile_serialization_roundtrip_preserves_semantics() {
    let profile = profile();
    let yaml = serde_yaml_ng::to_string(&profile).unwrap();
    assert_eq!(serde_yaml_ng::from_str::<Profile>(&yaml).unwrap(), profile);
    assert!(yaml.contains("certificate_policy: tofu"));
    assert!(yaml.contains("endpoint: 10.0.0.111:3389"));
}

#[test]
fn connection_plans_are_pure_data_for_direct_and_ssh_routes() {
    let profile = profile();
    for route in [
        PlannedRoute::Direct,
        PlannedRoute::SshTunnel {
            jump_host: "repair".into(),
            target: profile.endpoint.clone(),
        },
    ] {
        let plan = ConnectionPlan {
            target: profile.endpoint.clone(),
            route,
            identity: profile.identity.clone(),
            display: profile.display.clone(),
            devices: profile.devices.clone(),
            security: profile.security.clone(),
            credentials: ResolvedCredentials::default(),
            client: client(),
        };
        assert_eq!(plan.security.certificate_policy, CertificatePolicy::Tofu);
        assert_eq!(plan.target, profile.endpoint);
    }
}

#[test]
fn connection_plan_source_contains_no_live_resource_types() {
    let source =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/model/plan.rs"))
            .unwrap();
    for forbidden in ["Child", "Process", "Socket", "TcpStream", "UnixStream"] {
        assert!(
            !source.contains(forbidden),
            "INV-1 violation: plan.rs contains {forbidden}"
        );
    }
}
