use rdp_tui::model::SessionId;
use rdp_tui::runtime::process::{LaunchMode, spawn_child};
use rdp_tui::runtime::registry::ChildKind;
use std::process::Command;

fn session() -> SessionId {
    "550e8400-e29b-41d4-a716-446655440000"
        .parse::<SessionId>()
        .unwrap()
}

fn process_group_of(pid: u32) -> u32 {
    // /proc/<pid>/stat field 5 (pgrp) is the third whitespace token after the
    // "(comm) " prefix (state, ppid, pgrp, ...).
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    let tail = stat.rsplit_once(") ").unwrap().1;
    tail.split_whitespace().nth(2).unwrap().parse().unwrap()
}

#[test]
fn spawned_child_captures_identity_and_can_be_terminated() {
    let mut command = Command::new("sleep");
    command.arg("5");
    let mut child = spawn_child(
        &mut command,
        ChildKind::FreeRdp,
        session(),
        LaunchMode::OneShot,
    )
    .unwrap();
    assert!(child.terminate_if_owned().unwrap().is_some());
}

#[test]
fn detached_child_leads_its_own_process_group() {
    let mut command = Command::new("sleep");
    command.arg("5");
    let mut child = spawn_child(
        &mut command,
        ChildKind::FreeRdp,
        session(),
        LaunchMode::Detached,
    )
    .unwrap();
    let pid = child.child.id();
    assert_eq!(
        process_group_of(pid),
        pid,
        "a detached child should lead its own process group"
    );
    child.terminate_if_owned().unwrap();
}

#[test]
fn one_shot_child_stays_in_the_caller_process_group() {
    let mut command = Command::new("sleep");
    command.arg("5");
    let mut child = spawn_child(
        &mut command,
        ChildKind::FreeRdp,
        session(),
        LaunchMode::OneShot,
    )
    .unwrap();
    let pid = child.child.id();
    assert_eq!(
        process_group_of(pid),
        process_group_of(std::process::id()),
        "a one-shot child should stay in the caller's process group"
    );
    child.terminate_if_owned().unwrap();
}
