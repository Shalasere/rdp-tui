use rdp_tui::cli::commands::run;
use tempfile::TempDir;

#[test]
fn empty_store_has_inspection_commands_and_reports_validation() {
    let temporary = TempDir::new().expect("temporary config directory");
    let root = temporary.path().to_path_buf();
    assert_eq!(run(&[], &root).expect("list"), "");
    assert_eq!(
        run(&["validate".into()], &root).expect("validate"),
        "valid: 0 profile(s)\n"
    );
    let paths = run(&["config-paths".into()], &root).expect("paths");
    assert!(paths.contains("config.toml"));
    assert!(paths.contains("profiles.toml"));
}

#[test]
fn invalid_commands_are_explained_without_touching_configuration() {
    let temporary = TempDir::new().expect("temporary config directory");
    let root = temporary.path().to_path_buf();
    let error = run(&["connect".into()], &root).expect_err("unsupported command");
    assert!(error.starts_with("usage:"));
    assert!(!temporary.path().join("profiles.toml").exists());
}

#[test]
fn doctor_is_read_only_and_reports_each_renderer() {
    let temporary = TempDir::new().expect("temporary config directory");
    let root = temporary.path().to_path_buf();
    let output = run(&["doctor".into()], &root).expect("doctor");
    assert!(output.contains("wayland_sdl:"));
    assert!(output.contains("x11:"));
    assert!(!root.join("profiles.toml").exists());
}
