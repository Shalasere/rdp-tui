use rdp_tui::freerdp::discover::parse_version;
use semver::Version;
#[test]
fn parses_standard_freerdp_version_output() {
    assert_eq!(
        parse_version("This is FreeRDP version 3.30.0 (abc)"),
        Some(Version::new(3, 30, 0))
    );
}
