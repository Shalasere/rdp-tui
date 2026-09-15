use rdp_tui::model::SessionId;
use rdp_tui::runtime::process::{LaunchMode, spawn_child};
use rdp_tui::runtime::registry::{
    ChildKind, DiscoveredProcess, ProcessObservedState, deregister, observe, register,
    scan_for_orphans, still_matches,
};
use std::process::Command;
use std::time::{Duration, Instant};

fn session() -> SessionId {
    "550e8400-e29b-41d4-a716-446655440000"
        .parse::<SessionId>()
        .unwrap()
}

#[test]
fn process_identity_requires_pid_start_time_and_uid() {
    let id = observe(std::process::id(), ChildKind::FreeRdp, session()).unwrap();
    assert!(still_matches(id));
}

#[test]
fn a_live_registered_child_scans_as_still_running() {
    let me = observe(std::process::id(), ChildKind::FreeRdp, session()).unwrap();
    register(me);
    let scanned = scan_for_orphans();
    let entry = scanned
        .iter()
        .find(|discovered: &&DiscoveredProcess| discovered.identity == me)
        .expect("registered identity appears in the scan");
    assert_eq!(entry.observed_state, ProcessObservedState::StillRunning);
    assert_eq!(deregister(me.pid), Some(me));
    assert!(scan_for_orphans().iter().all(|d| d.identity != me));
}

#[test]
fn a_reaped_registered_child_scans_as_stale() {
    let mut command = Command::new("sleep");
    command.arg("5");
    let mut child = spawn_child(
        &mut command,
        ChildKind::Tunnel,
        session(),
        LaunchMode::OneShot,
    )
    .unwrap();
    let identity = child.identity;
    register(identity);
    // Kill and reap the child, then confirm the registry reports it as gone.
    child.terminate_if_owned().unwrap();
    let scanned = scan_for_orphans();
    let entry = scanned
        .iter()
        .find(|discovered: &&DiscoveredProcess| discovered.identity == identity)
        .expect("registered identity appears in the scan");
    assert_eq!(entry.observed_state, ProcessObservedState::Stale);
    let _ = deregister(identity.pid);
}

#[test]
fn a_zombie_is_not_reported_as_running() {
    let mut child = Command::new("true").spawn().unwrap();
    let pid = child.id();
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
        let state = stat.rsplit_once(") ").unwrap().1.split_whitespace().next();
        if state == Some("Z") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "child {pid} did not exit in time"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    assert!(
        observe(pid, ChildKind::Supervisor, session()).is_err(),
        "a zombie has exited and must not be reported as live"
    );
    child.wait().unwrap();
}
