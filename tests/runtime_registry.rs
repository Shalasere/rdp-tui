use rdp_tui::model::SessionId;
use rdp_tui::runtime::registry::{ChildKind, observe, still_matches};
#[test]
fn process_identity_requires_pid_start_time_and_uid() {
    let id = observe(
        std::process::id(),
        ChildKind::FreeRdp,
        "550e8400-e29b-41d4-a716-446655440000"
            .parse::<SessionId>()
            .unwrap(),
    )
    .unwrap();
    assert!(still_matches(id));
}
