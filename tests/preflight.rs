use rdp_tui::model::{
    ConnectionPlan, DeviceConfig, DisplayConfig, FreeRdpClient, IdentityConfig, PlannedRoute,
    Renderer, ResolvedCredentials, SecurityConfig,
};
use rdp_tui::preflight::check_tcp;
use rdp_tui::preflight::prepare;
use semver::Version;
use std::net::TcpListener;
use std::path::PathBuf;
use std::time::Duration;

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

#[test]
fn tcp_check_distinguishes_a_reachable_listener_from_a_closed_port() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("127.0.0.1:{}", listener.local_addr().unwrap().port())
        .parse()
        .unwrap();
    assert_eq!(check_tcp(&endpoint, Duration::from_millis(100)), Ok(()));

    drop(listener);
    assert!(check_tcp(&endpoint, Duration::from_millis(100)).is_err());
}
