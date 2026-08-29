use rdp_tui::model::SessionId;
use rdp_tui::runtime::process::spawn_child;
use rdp_tui::runtime::registry::ChildKind;
use std::process::Command;
#[test]
fn spawned_child_captures_identity_and_can_be_terminated() {
    let mut command = Command::new("sleep");
    command.arg("5");
    let session = "550e8400-e29b-41d4-a716-446655440000"
        .parse::<SessionId>()
        .unwrap();
    let mut child = spawn_child(&mut command, ChildKind::FreeRdp, session).unwrap();
    assert!(child.terminate_if_owned().unwrap().is_some());
}
