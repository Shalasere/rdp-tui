use rdp_tui::freerdp::command::build_command;
use rdp_tui::model::{
    CertificatePolicy, ConnectionPlan, DeviceConfig, DisplayConfig, FreeRdpClient, IdentityConfig,
    PlannedRoute, Renderer, ResolvedCredentials, SecurityConfig,
};
use rdp_tui::preflight::prepare;
use semver::Version;
use std::path::PathBuf;

#[test]
fn command_uses_prepared_endpoint_and_never_credential_references() {
    let mut plan = ConnectionPlan {
        target: "anima:3389".parse().unwrap(),
        route: PlannedRoute::Direct,
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
    };
    plan.display.fullscreen = true;
    plan.display.multimon = true;
    plan.display.resolution = Some((1920, 1080));
    plan.security.certificate_policy = CertificatePolicy::Tofu;
    let mut prepared = prepare(&plan).unwrap();
    prepared.effective_endpoint = "tofu:3390".parse().unwrap();
    let (_, args, environment) = build_command(&prepared);
    let args = args
        .iter()
        .map(|arg| arg.to_string_lossy())
        .collect::<Vec<_>>();
    assert!(args.contains(&"/v:tofu:3390".into()));
    assert!(!args.contains(&"/v:anima:3389".into()));
    assert!(args.contains(&"/cert:tofu".into()));
    assert!(args.contains(&"/multimon".into()));
    assert!(args.contains(&"/size:1920x1080".into()));
    // X11 fullscreen still uses FreeRDP's own /f.
    assert!(args.contains(&"/f".into()));
    assert!(environment.is_empty());
}

#[test]
fn command_renders_supported_profile_settings() {
    let mut plan = ConnectionPlan {
        target: "anima:3389".parse().unwrap(),
        route: PlannedRoute::Direct,
        identity: IdentityConfig {
            username: "local-user".into(),
            domain: String::new(),
        },
        display: DisplayConfig::default(),
        devices: DeviceConfig::default(),
        security: SecurityConfig::default(),
        credentials: ResolvedCredentials::default(),
        client: FreeRdpClient {
            executable: PathBuf::from("xfreerdp3"),
            renderer: Renderer::X11,
            version: Version::new(3, 30, 0),
        },
    };
    plan.display.span_monitors = true;
    plan.display.smart_sizing = true;
    plan.display.scale_percent = Some(140);
    plan.display.color_depth = Some(32);
    plan.devices.microphone = true;
    plan.devices.shared_folders = vec![PathBuf::from("/srv/shared")];
    plan.security.admin_session = true;

    let prepared = prepare(&plan).unwrap();
    let (_, args, _) = build_command(&prepared);
    let args = args
        .iter()
        .map(|arg| arg.to_string_lossy())
        .collect::<Vec<_>>();

    for expected in [
        "/auth-pkg-list:none,ntlm",
        "/sound",
        "/microphone",
        "/drive:rdp-tui-1,/srv/shared",
        "/admin",
        "/span",
        "/smart-sizing",
        "/scale:140",
        "/bpp:32",
    ] {
        assert!(
            args.contains(&expected.into()),
            "missing {expected}: {args:?}"
        );
    }
    assert!(!args.contains(&"/multimon".into()));
}

#[test]
fn sdl_fullscreen_uses_explicit_size_not_the_fullscreen_flag() {
    let mut plan = ConnectionPlan {
        target: "anima:3389".parse().unwrap(),
        route: PlannedRoute::Direct,
        identity: IdentityConfig::default(),
        display: DisplayConfig::default(),
        devices: DeviceConfig::default(),
        security: SecurityConfig::default(),
        credentials: ResolvedCredentials::default(),
        client: FreeRdpClient {
            executable: PathBuf::from("sdl-freerdp3"),
            renderer: Renderer::WaylandSdl,
            version: Version::new(3, 30, 0),
        },
    };
    plan.display.fullscreen = true;
    plan.display.resolution = Some((2560, 1440));
    let prepared = prepare(&plan).unwrap();
    let (_, args, _) = build_command(&prepared);
    let args = args
        .iter()
        .map(|arg| arg.to_string_lossy())
        .collect::<Vec<_>>();
    // SDL must not use /f (it mis-detects a 64x64 monitor); it uses /size instead.
    assert!(
        !args.contains(&"/f".into()),
        "SDL must not use /f: {args:?}"
    );
    assert!(args.contains(&"/size:2560x1440".into()));
    assert!(args.contains(&"-grab-keyboard".into()));
    assert!(args.contains(&"-grab-mouse".into()));
}
