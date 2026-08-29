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

#[test]
fn python_migration_upserts_profiles_without_modifying_the_source() {
    let temporary = TempDir::new().expect("temporary config directory");
    let root = temporary.path().to_path_buf();
    let source = root.join("legacy.json");
    let json =
        r#"[{"id":"550e8400-e29b-41d4-a716-446655440000","name":"Anima","host":"10.0.0.111"}]"#;
    std::fs::write(&source, json).unwrap();
    assert_eq!(
        run(
            &["migrate".into(), "python".into(), "legacy.json".into()],
            &root
        )
        .unwrap(),
        "migrated: 1 profile(s); secrets were not migrated\n"
    );
    assert_eq!(std::fs::read_to_string(source).unwrap(), json);
    assert!(run(&["list".into()], &root).unwrap().contains("Anima"));
}

#[test]
fn inspect_reports_a_plan_without_preparing_a_connection() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path().to_path_buf();
    std::fs::write(root.join("legacy.json"), r#"[{"id":"550e8400-e29b-41d4-a716-446655440000","name":"Anima","host":"10.0.0.111","renderer":"x11"}]"#).unwrap();
    run(
        &["migrate".into(), "python".into(), "legacy.json".into()],
        &root,
    )
    .unwrap();
    let result = run(
        &[
            "inspect".into(),
            "550e8400-e29b-41d4-a716-446655440000".into(),
        ],
        &root,
    );
    match result {
        Ok(output) => {
            assert!(output.contains("profile: Anima"));
            assert!(output.contains("target: 10.0.0.111:3389"));
        }
        Err(error) => assert_eq!(error, "FreeRDP frontend is not installed"),
    }
    assert!(!root.join("sessions").exists());
}
