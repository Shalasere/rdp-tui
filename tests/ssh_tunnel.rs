use rdp_tui::ssh::tunnel::command;
#[test]
fn tunnel_command_is_noninteractive_and_uses_local_forward() {
    let args = command("jump", 44444, &"anima:3389".parse().unwrap())
        .into_iter()
        .map(|v| v.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    assert!(args.windows(2).any(|item| item == ["-o", "BatchMode=yes"]));
    assert!(
        args.windows(2)
            .any(|item| item == ["-o", "StrictHostKeyChecking=accept-new"])
    );
    assert!(args.contains(&"127.0.0.1:44444:anima:3389".into()));
    assert_eq!(args.last(), Some(&"jump".into()));
}
