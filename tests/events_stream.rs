//! 事件流：两个订阅者各收到一次、按 kind 过滤、客户端断开即从登记表移除、
//! 队列溢出补发 `events.dropped`。直接驱动控制 socket，不依赖 core。
#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use hc_tui::config::Config;
use hc_tui::control::Server;

fn config() -> Config {
    // unix socket 路径上限 104 字节（macOS）：`env::temp_dir()` 在 macOS 上是
    // `/var/folders/…/T`，再加会话根与 `state/control-<hash>.sock` 就超限，
    // 所以用例自己起短根。
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let unique = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let root =
        std::path::Path::new("/tmp").join(format!("hctui-ev-{}-{unique}", std::process::id()));
    std::fs::create_dir_all(root.join("state")).unwrap();
    Config {
        config_path: root.join("config").join("config.toml"),
        state_path: root.join("state").join("state.json"),
        restore: false,
        ..Config::default()
    }
}

/// UI 侧消费者：只回答普通请求，保持命令通道可用；快照带当前 revision。
fn serve(
    receiver: std::sync::mpsc::Receiver<hc_tui::control::Request>,
    publisher: hc_tui::control::Publisher,
) {
    std::thread::spawn(move || {
        for request in receiver {
            let response = if request.body["method"] == json!("snapshot") {
                json!({"state": {}, "revision": publisher.revision()})
            } else {
                json!({"ok": true, "echo": request.body})
            };
            let _ = request.reply.send(response);
        }
    });
}

fn subscribe(config: &Config, request: Value) -> BufReader<UnixStream> {
    let mut stream = UnixStream::connect(hc_tui::control::socket_path(config)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    writeln!(stream, "{request}").unwrap();
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let ack: Value = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(ack["subscribed"], json!(true), "{ack}");
    reader
}

fn next_event(reader: &mut BufReader<UnixStream>) -> Value {
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    assert!(!line.is_empty(), "event stream closed");
    serde_json::from_str(line.trim()).unwrap()
}

fn wait_for(predicate: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if predicate() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

#[test]
fn two_clients_each_get_one_copy_and_disconnect_stops_the_subscription() {
    let config = config();
    let (server, receiver, publisher) = Server::start(&config).unwrap();
    serve(receiver, publisher.clone());
    let mut status_only = subscribe(
        &config,
        json!({"method": "events", "kinds": ["terminal.status"]}),
    );
    let mut everything = subscribe(&config, json!({"method": "events"}));
    assert!(wait_for(|| publisher.subscribers() == 2));

    publisher.publish(&json!({"kind": "terminal.status", "terminal": "t1", "turn": 1}));
    assert_eq!(next_event(&mut status_only)["kind"], "terminal.status");
    assert_eq!(next_event(&mut everything)["kind"], "terminal.status");

    publisher.publish(&json!({"kind": "terminal.turn", "terminal": "t1", "turn": 2}));
    assert_eq!(next_event(&mut everything)["kind"], "terminal.turn");
    // 过滤掉的事件不会到达 status_only。
    status_only
        .get_ref()
        .set_read_timeout(Some(Duration::from_millis(150)))
        .unwrap();
    let mut line = String::new();
    let _ = status_only.read_line(&mut line);
    assert!(line.is_empty(), "unexpected filtered event: {line}");

    drop(status_only);
    assert!(wait_for(|| publisher.subscribers() == 1));
    publisher.publish(&json!({"kind": "terminal.status", "terminal": "t1", "turn": 3}));
    assert_eq!(next_event(&mut everything)["kind"], "terminal.status");
    drop(everything);
    drop(server);
}

#[test]
fn overflow_drops_and_reports_events_dropped() {
    let config = config();
    let (server, receiver, publisher) = Server::start(&config).unwrap();
    serve(receiver, publisher.clone());
    let mut reader = subscribe(&config, json!({"method": "events"}));
    // 订阅者先不读，填满队列与 socket 缓冲。
    let filler = "x".repeat(200);
    let total = 50_000;
    for index in 0..total {
        publisher.publish(&json!({
            "kind": "terminal.status",
            "terminal": "t1",
            "turn": index,
            "data": {"filler": filler},
        }));
    }
    let mut delivered = 0u64;
    let mut dropped = None;
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        let event = next_event(&mut reader);
        if event["kind"] == json!("events.dropped") {
            dropped = event["count"].as_u64();
            break;
        }
        delivered += 1;
    }
    let dropped = dropped.expect("events.dropped was never sent");
    assert!(dropped > 0, "dropped count: {dropped}");
    assert!(
        delivered >= 1,
        "no event was delivered before the drop notice"
    );
    assert!(
        delivered + dropped <= total as u64,
        "delivered {delivered} + dropped {dropped} exceeds {total}"
    );
    // 丢弃通知之后流仍然可用。
    publisher.publish(&json!({"kind": "terminal.status", "terminal": "t1", "turn": 0}));
    assert_eq!(next_event(&mut reader)["kind"], "terminal.status");
    drop(reader);
    drop(server);
}

#[test]
fn since_sends_one_snapshot_then_only_newer_events() {
    let config = config();
    let (server, receiver, publisher) = Server::start(&config).unwrap();
    serve(receiver, publisher.clone());
    for turn in 1..=3 {
        publisher.publish(&json!({"kind": "terminal.status", "terminal": "t1", "turn": turn}));
    }
    let mut reader = subscribe(&config, json!({"method": "events", "since": 2}));
    let first = next_event(&mut reader);
    assert_eq!(first["kind"], json!("events.snapshot"), "{first}");
    assert_eq!(first["resync"], json!(false), "{first}");
    assert_eq!(first["revision"], json!(3), "{first}");
    println!(
        "first frame: kind={} revision={} resync={}",
        first["kind"], first["revision"], first["resync"]
    );
    // 快照已经把 1..=3 都包进去了，不重发旧 revision。
    reader
        .get_ref()
        .set_read_timeout(Some(Duration::from_millis(150)))
        .unwrap();
    let mut line = String::new();
    let _ = reader.read_line(&mut line);
    assert!(line.is_empty(), "unexpected replayed event: {line}");
    reader
        .get_ref()
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    publisher.publish(&json!({"kind": "terminal.status", "terminal": "t1", "turn": 4}));
    let live = next_event(&mut reader);
    assert_eq!(live["turn"], json!(4), "{live}");
    assert_eq!(live["revision"], json!(4), "{live}");
    println!(
        "live frame: revision={} turn={}",
        live["revision"], live["turn"]
    );
    drop(reader);
    drop(server);
}

#[test]
fn a_stale_or_future_since_resyncs_from_the_snapshot() {
    let config = config();
    let (server, receiver, publisher) = Server::start(&config).unwrap();
    serve(receiver, publisher.clone());
    let total = hc_tui::control::HISTORY + 5;
    for turn in 0..total {
        publisher.publish(&json!({"kind": "terminal.status", "terminal": "t1", "turn": turn}));
    }
    let mut stale = subscribe(&config, json!({"method": "events", "since": 1}));
    let first = next_event(&mut stale);
    assert_eq!(first["kind"], json!("events.snapshot"), "{first}");
    assert_eq!(first["resync"], json!(true), "{first}");
    assert_eq!(first["revision"], json!(total), "{first}");
    println!(
        "history {} events, since=1: kind={} revision={} resync={}",
        total, first["kind"], first["revision"], first["resync"]
    );
    drop(stale);

    let mut future = subscribe(&config, json!({"method": "events", "since": total + 10}));
    let first = next_event(&mut future);
    assert_eq!(first["resync"], json!(true), "{first}");
    println!(
        "since={}: kind={} revision={} resync={}",
        total + 10,
        first["kind"],
        first["revision"],
        first["resync"]
    );
    drop(future);
    drop(server);
}
