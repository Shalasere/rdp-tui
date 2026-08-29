use rdp_tui::model::{
    ConnectionPlan, DeviceConfig, DisplayConfig, FreeRdpClient, IdentityConfig, PlannedRoute,
    Renderer, ResolvedCredentials, SecurityConfig,
};
use rdp_tui::preflight::prepare;
use semver::Version;
use std::path::PathBuf;

fn plan(route: PlannedRoute) -> ConnectionPlan {
    ConnectionPlan {
        target: "anima:3389".parse().unwrap(),
        route,
        identity: IdentityConfig::default(),
        display: DisplayConfig::default(),
        devices: DeviceConfig::default(),
        security: SecurityConfig::default(),
        credentials: ResolvedCredentials::default(),
        client: FreeRdpClient {
            executable: PathBuf::from("xfreerdp3"),
            renderer: Renderer::X11,
            version: Version::new(3, 30, 0),
        },
    }
}

#[test]
fn direct_preparation_has_no_live_route_handle() {
    let prepared = prepare(&plan(PlannedRoute::Direct)).unwrap();
    assert_eq!(prepared.effective_endpoint.to_string(), "anima:3389");
    assert!(prepared.route_handle.is_none());
}

#[test]
fn ssh_preparation_is_explicitly_unsupported_until_tunnels_exist() {
    assert!(
        prepare(&plan(PlannedRoute::SshTunnel {
            jump_host: "jump".into(),
            target: "anima:3389".parse().unwrap()
        }))
        .is_err()
    );
}
