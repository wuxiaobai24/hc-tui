//! M7 插件终端的协议级验收：远端 `terminal.run` 的 argv/cwd/env、标签、
//! 关闭视图后程序继续运行，以及 `requires_programs` 的目标机器探测。
//!
//! 需要已安装 `hc`、`ssh` 能免密到 localhost，默认跳过：
//!
//! ```sh
//! HC_TUI_E2E=1 cargo test -p hc-tui --test plugin_terminals -- --nocapture
//! ```
//!
//! 客户端会话与 peer 会话都在 `/tmp/hc-tui-e2e-plugin` 下隔离；peer 是
//! localhost 上的独立 core session，结束时停掉。

use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use anyhow::Result;
use base64::{engine::general_purpose::STANDARD, Engine};
use ratatui::{buffer::Buffer, layout::Rect};

use hc_tui::hc::net::{Command, NetConfig, NetEvent, NetHandle};
use hc_tui::plugin::terminal;
use hc_tui::term::view::TerminalView;

const SESSION: &str = "hctui-m7-e2e";
const PEER_SESSION: &str = "hctui-m7-e2e-peer";

fn isolated_environment() {
    let root = Path::new("/tmp/hc-tui-e2e-plugin");
    for (key, name) in [
        ("XDG_RUNTIME_DIR", "run"),
        ("XDG_STATE_HOME", "state"),
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_CACHE_HOME", "cache"),
    ] {
        let path = root.join(name);
        std::fs::create_dir_all(&path).unwrap();
        std::env::set_var(key, path);
    }
    std::env::set_var("HC_SESSION", SESSION);
}

/// 测试结束时停掉 peer 的 core；它会跑在本机默认 XDG 的独立会话里。
struct PeerGuard;

impl Drop for PeerGuard {
    fn drop(&mut self) {
        stop_peer_core();
        // core 退出前可能重写会话目录：停稳后再删，并重试一次。
        std::thread::sleep(Duration::from_millis(400));
        for _ in 0..2 {
            let Some(dir) = peer_session_dir() else {
                break;
            };
            let _ = std::fs::remove_dir_all(&dir);
            if !dir.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(400));
        }
    }
}

fn stop_peer_core() {
    let _ = std::process::Command::new("hc")
        .args(["server", "stop"])
        .env_remove("XDG_RUNTIME_DIR")
        .env_remove("XDG_STATE_HOME")
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_CACHE_HOME")
        .env("HC_SESSION", PEER_SESSION)
        .output();
}

fn peer_session_dir() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(
        Path::new(&home)
            .join(".local/state/herdr-core")
            .join(PEER_SESSION),
    )
}

/// 上一次运行可能留下了 peer 的 core；开跑前先清掉，保证 machine add 能重新探测。
fn reset_peer_session() {
    stop_peer_core();
    std::thread::sleep(Duration::from_millis(300));
    if let Some(dir) = peer_session_dir() {
        let _ = std::fs::remove_dir_all(dir);
    }
}

fn add_peer() -> bool {
    // 客户端会话在隔离的 state 目录里跨运行保留：旧条目先移除再重加。
    let _ = std::process::Command::new("hc")
        .args(["machine", "rm", "peer"])
        .output();
    let output = std::process::Command::new("hc")
        .args([
            "machine",
            "add",
            "localhost",
            "--name",
            "peer",
            "--session",
            PEER_SESSION,
            "--install",
            "--json",
        ])
        .output();
    match output {
        Ok(output) if output.status.success() => true,
        Ok(output) => {
            eprintln!(
                "hc machine add failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
            false
        }
        Err(error) => {
            eprintln!("hc machine add failed: {error}");
            false
        }
    }
}

struct Harness {
    net: NetHandle,
    events: Receiver<NetEvent>,
    streams: HashMap<u32, u32>,
    views: HashMap<u32, TerminalView>,
    notes: Vec<String>,
}

impl Harness {
    fn new() -> Self {
        let (sender, events) = std::sync::mpsc::channel();
        let net = NetHandle::spawn(
            NetConfig {
                session: Some(SESSION.to_string()),
            },
            sender,
        );
        Self {
            net,
            events,
            streams: HashMap::new(),
            views: HashMap::new(),
            notes: Vec::new(),
        }
    }

    fn handle(&mut self, event: NetEvent) {
        match event {
            NetEvent::Attached {
                pane,
                channel,
                terminal,
                cols,
                rows,
                ..
            } => {
                self.notes
                    .push(format!("attached {terminal} {cols}x{rows}"));
                self.streams.insert(pane, channel);
                let view = self
                    .views
                    .entry(pane)
                    .or_insert_with(|| TerminalView::new(cols, rows, 1 << 20).unwrap());
                view.resize(cols, rows, 8, 16).unwrap();
            }
            NetEvent::Output { channel, bytes } => {
                let pane = self
                    .streams
                    .iter()
                    .find(|(_, stream)| **stream == channel)
                    .map(|(pane, _)| *pane);
                if let Some(pane) = pane {
                    let responses = match self.views.get_mut(&pane) {
                        Some(view) => {
                            view.write(&bytes);
                            view.take_responses()
                        }
                        None => Vec::new(),
                    };
                    if !responses.is_empty() {
                        self.net.send(Command::Input {
                            pane,
                            bytes: responses,
                        });
                    }
                }
                self.net.send(Command::Credit {
                    channel,
                    bytes: bytes.len(),
                });
            }
            NetEvent::AttachClosed { pane, reason, .. } => {
                self.notes.push(format!("closed {pane}: {reason}"));
            }
            NetEvent::Failed { reason } => panic!("net thread failed: {reason}"),
            _ => {}
        }
    }

    fn wait_until(&mut self, what: &str, mut done: impl FnMut(&mut Harness) -> bool) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if done(&mut *self) {
                return Ok(());
            }
            if Instant::now() > deadline {
                anyhow::bail!("timed out waiting for {what}; notes: {:?}", self.notes);
            }
            if let Ok(event) = self.events.recv_timeout(Duration::from_millis(50)) {
                self.handle(event);
            }
        }
    }

    fn screen(&mut self, pane: u32) -> String {
        let Some(view) = self.views.get_mut(&pane) else {
            return String::new();
        };
        let channel = self.streams[&pane];
        let _ = channel;
        let area = Rect::new(0, 0, 80, 24);
        let mut buffer = Buffer::empty(area);
        if view.render(&mut buffer, area).is_err() {
            return String::new();
        }
        let mut out = String::new();
        for y in 0..area.height {
            for x in 0..area.width {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }
}

fn address_of(harness: &Harness) -> String {
    harness
        .notes
        .iter()
        .find_map(|note| note.strip_prefix("attached ").map(str::to_string))
        .and_then(|rest| rest.split_whitespace().next().map(str::to_string))
        .unwrap_or_default()
}

#[test]
fn remote_terminal_keeps_argv_cwd_env_and_survives_view_close() -> Result<()> {
    if std::env::var("HC_TUI_E2E").is_err() {
        eprintln!("skipping M7 end-to-end test; set HC_TUI_E2E=1 to run it against a real core");
        return Ok(());
    }
    isolated_environment();
    reset_peer_session();
    let _peer = PeerGuard;
    if !add_peer() {
        eprintln!("skipping M7 end-to-end test; ssh to localhost is not available for a peer");
        return Ok(());
    }
    let mut harness = Harness::new();
    harness.net.send(Command::Spawn {
        keep: true,
        pane: 1,
        target: None,
        argv: vec![
            "sh".into(),
            "-c".into(),
            "echo FOO=$FOO; pwd; sleep 30".into(),
        ],
        cwd: Some("/tmp".into()),
        on: Some("peer".into()),
        env: [("FOO".to_string(), "bar".to_string())]
            .into_iter()
            .collect(),
        cols: 80,
        rows: 24,
    });
    harness.wait_until("remote attach", |harness| harness.streams.contains_key(&1))?;
    harness.wait_until("terminal output", |harness| {
        let screen = harness.screen(1);
        screen.contains("FOO=bar") && screen.contains("/tmp")
    })?;
    let address = address_of(&harness);
    assert!(
        address.starts_with("peer/"),
        "remote terminal address: {address}"
    );
    println!("remote terminal {address} runs env and cwd as planned");

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let mut conn = hc_tui::hc::client::Conn::connect(Some(SESSION)).await?;
        let label = conn
            .call(
                "terminal.label",
                serde_json::json!({
                    "terminal": address,
                    "labels": {
                        "hctui.plugin": "dev.hctui.plugins.git-tools",
                        "hctui.terminal": "git-ui",
                        "where": "api / 1"
                    }
                }),
            )
            .await?;
        println!("label reply address {}", label["address"]);
        let record = conn
            .call("terminal.get", serde_json::json!({"terminal": address}))
            .await?;
        assert_eq!(
            record.pointer("/labels/hctui.plugin"),
            Some(&serde_json::json!("dev.hctui.plugins.git-tools"))
        );
        assert_eq!(
            record.pointer("/labels/hctui.terminal"),
            Some(&serde_json::json!("git-ui"))
        );
        assert_eq!(record["cwd"], serde_json::json!("/tmp"));
        assert_eq!(record["keep"], serde_json::json!(true));

        // requires_programs 在目标机器上检查：只有缺失的程序出现在输出里。
        let programs = vec![
            "lazygit".to_string(),
            "definitely-missing-hctui-prog".to_string(),
        ];
        let probe = conn
            .call(
                "machine.exec",
                terminal::probe_params(&programs, "peer", 20),
            )
            .await?;
        let stdout = STANDARD
            .decode(probe["stdout"].as_str().unwrap_or_default())
            .unwrap_or_default();
        let stdout = String::from_utf8_lossy(&stdout);
        let missing = terminal::parse_missing(&stdout, &programs);
        assert_eq!(missing, vec!["definitely-missing-hctui-prog".to_string()]);
        println!("probe on peer reports {missing:?}");

        // 关闭视图：程序仍在 core 里运行。
        harness.net.send(Command::Close { pane: 1 });
        std::thread::sleep(Duration::from_millis(500));
        let after = conn
            .call("terminal.get", serde_json::json!({"terminal": address}))
            .await?;
        assert_eq!(after["state"], serde_json::json!("live"));
        println!("terminal {address} is still live after the view closed");
        conn.call("terminal.kill", serde_json::json!({"terminal": address}))
            .await?;
        Ok::<(), anyhow::Error>(())
    })?;
    Ok(())
}
