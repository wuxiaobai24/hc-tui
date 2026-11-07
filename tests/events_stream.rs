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
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    let root = std::env::temp_dir().join(format!("hctui-events-{unique}"));
    std::fs::create_dir_all(root.join("state")).unwrap();
    Config {
        config_path: root.join("config").join("config.toml"),
        state_path: root.join("state").join("state.json"),
        restore: false,
        ..Config::default()
    }
}

/// UI 侧消费者：只回答普通请求，保持命令通道可用。
fn serve(receiver: std::sync::mpsc::Receiver<hc_tui::control::Request>) {
    std::thread::spawn(move || {
        for request in receiver {
            let _ = request
                .reply
                .send(json!({"ok": true, "echo": request.body}));
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
    serve(receiver);
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
    serve(receiver);
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
