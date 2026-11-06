//! Fault injection uses a fake hc in this test process only; no user core is touched.
#![cfg(unix)]
use hc_tui::hc::net::{NetConfig, NetEvent, NetHandle};
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

#[test]
fn hello_eof_retries_and_shutdown_cancels_a_stalled_handshake() {
    let root = std::env::temp_dir().join(format!("hctui-net-fault-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let executable = root.join("hc");
    let marker = root.join("attempt");
    let stalled = root.join("stalled");
    std::fs::write(&executable, format!(
        "#!/bin/sh\nread request\nif [ ! -f '{}' ]; then : > '{}'; exit 1; fi\n: > '{}'\nexec /bin/sleep 3\n",
        marker.display(), marker.display(), stalled.display()
    )).unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    let old_path = std::env::var_os("PATH");
    std::env::set_var("PATH", &root);
    let (sender, events) = std::sync::mpsc::channel();
    let mut net = NetHandle::spawn(NetConfig { session: None }, sender);
    let result = events.recv_timeout(Duration::from_secs(2));
    let deadline = Instant::now() + Duration::from_secs(2);
    while !stalled.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let did_retry = stalled.exists();
    let start = Instant::now();
    net.shutdown();
    let shutdown_time = start.elapsed();
    match old_path {
        Some(path) => std::env::set_var("PATH", path),
        None => std::env::remove_var("PATH"),
    }
    let _ = std::fs::remove_dir_all(root);
    assert!(
        matches!(result, Ok(NetEvent::Reconnecting { .. })),
        "{result:?}"
    );
    assert!(did_retry, "EOF must retry the connection");
    assert!(
        shutdown_time < Duration::from_millis(500),
        "shutdown took {shutdown_time:?}"
    );
}
