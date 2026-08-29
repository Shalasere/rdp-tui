use rdp_tui::config::migrate::import_python_profiles;
use rdp_tui::model::{CertificatePolicy, Renderer, Route};

#[test]
fn imports_python_profiles_without_persisted_credentials() {
    let document = import_python_profiles(r#"[{"id":"550e8400-e29b-41d4-a716-446655440000","name":"Anima","host":"10.0.0.111","user":"ada","domain":"LAN","fullscreen":true,"clipboard":true,"audio":true,"resolution":"1920x1080","certificate_policy":"tofu","renderer":"x11","gateway_host":"gateway:443"}]"#).unwrap();
    let profile = &document.profiles[0];
    assert_eq!(profile.name, "Anima");
    assert_eq!(profile.endpoint.to_string(), "10.0.0.111:3389");
    assert_eq!(profile.display.renderer, Renderer::X11);
    assert_eq!(profile.security.certificate_policy, CertificatePolicy::Tofu);
    assert!(profile.credential.is_none());
    assert!(matches!(profile.route, Route::RdGateway { .. }));
}

#[test]
fn rejects_legacy_free_form_options_for_manual_review() {
    let error=import_python_profiles(r#"[{"id":"550e8400-e29b-41d4-a716-446655440000","name":"Anima","host":"10.0.0.111","extra_options":"/cert:ignore"}]"#).unwrap_err();
    assert!(error.to_string().contains("extra_options"));
}
