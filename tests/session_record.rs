use rdp_tui::model::{ProfileId, SessionId};
use rdp_tui::runtime::registry::{ChildKind, ProcessIdentity, observe};
use rdp_tui::session::record::{SessionRecord, SessionRecordState, read, remove, write};
use tempfile::TempDir;

fn identity(session: SessionId) -> ProcessIdentity {
    observe(std::process::id(), ChildKind::FreeRdp, session).unwrap()
}

#[test]
fn a_session_record_round_trips_through_the_runtime_directory() {
    let dir = TempDir::new().unwrap();
    let session = "550e8400-e29b-41d4-a716-446655440000"
        .parse::<SessionId>()
        .unwrap();
    let profile = "111e8400-e29b-41d4-a716-446655440000"
        .parse::<ProfileId>()
        .unwrap();
    let record = SessionRecord {
        session_id: session,
        profile_id: profile,
        supervisor: identity(session),
        freerdp: Some(identity(session)),
        tunnel: None,
        state: SessionRecordState::Running,
    };
    write(dir.path(), &record).unwrap();

    let loaded = read(dir.path(), session).unwrap().expect("record present");
    assert_eq!(loaded, record);
    assert!(loaded.tunnel.is_none());

    remove(dir.path(), session).unwrap();
    assert!(read(dir.path(), session).unwrap().is_none());
}

#[test]
fn reading_a_missing_record_is_none_and_removing_it_is_ok() {
    let dir = TempDir::new().unwrap();
    let session = "550e8400-e29b-41d4-a716-446655440000"
        .parse::<SessionId>()
        .unwrap();
    assert!(read(dir.path(), session).unwrap().is_none());
    remove(dir.path(), session).unwrap();
}
