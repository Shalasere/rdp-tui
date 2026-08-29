use rdp_tui::model::{ProfileId, SessionId};
use rdp_tui::runtime::process::{LaunchMode, spawn_child};
use rdp_tui::runtime::registry::{ChildKind, observe};
use rdp_tui::session::record::{SessionRecord, SessionRecordState, read, write};
use rdp_tui::session::{SessionHealth, scan_sessions};
use std::process::Command;
use tempfile::TempDir;

fn session() -> SessionId {
    "550e8400-e29b-41d4-a716-446655440000"
        .parse::<SessionId>()
        .unwrap()
}

fn profile() -> ProfileId {
    "111e8400-e29b-41d4-a716-446655440000"
        .parse::<ProfileId>()
        .unwrap()
}

#[test]
fn scan_reports_a_live_supervisor_as_running_and_keeps_the_record() {
    let dir = TempDir::new().unwrap();
    let supervisor = observe(std::process::id(), ChildKind::Supervisor, session()).unwrap();
    let record = SessionRecord {
        session_id: session(),
        profile_id: profile(),
        supervisor,
        freerdp: None,
        tunnel: None,
        state: SessionRecordState::Running,
    };
    write(dir.path(), &record).unwrap();

    let statuses = scan_sessions(dir.path());
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].health, SessionHealth::Running);
    assert!(read(dir.path(), session()).unwrap().is_some());
}

#[test]
fn scan_removes_a_record_whose_processes_are_all_gone() {
    let dir = TempDir::new().unwrap();
    let mut command = Command::new("sleep");
    command.arg("5");
    let mut child = spawn_child(
        &mut command,
        ChildKind::Supervisor,
        session(),
        LaunchMode::OneShot,
    )
    .unwrap();
    let supervisor = child.identity;
    child.terminate_if_owned().unwrap(); // kill and reap; the PID is now gone

    let record = SessionRecord {
        session_id: session(),
        profile_id: profile(),
        supervisor,
        freerdp: None,
        tunnel: None,
        state: SessionRecordState::Running,
    };
    write(dir.path(), &record).unwrap();

    let statuses = scan_sessions(dir.path());
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].health, SessionHealth::Stale);
    assert!(read(dir.path(), session()).unwrap().is_none());
}
