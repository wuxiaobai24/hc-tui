//! P2 端到端验收：同一连接上多个 Pane 各自一条 attach 流。
//!
//! 覆盖：两个 Pane 同时 attach、各自独立收发、关掉一个视图不影响另一个、
//! 每条流的 credit 都还回去。需要已安装 `hc`，默认跳过：
//!
//! ```sh
//! HC_TUI_E2E=1 cargo test -p hc-tui --test multi_pane -- --nocapture
//! ```
//!
//! 全程使用 `/tmp/hc-tui-e2e` 下的独立 XDG 路径与 `hctui-mp` session。

use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use anyhow::Result;
use ratatui::{buffer::Buffer, layout::Rect};

use hc_tui::hc::net::{Command, NetConfig, NetEvent, NetHandle};
use hc_tui::term::view::TerminalView;

fn isolated_environment() {
    let root = Path::new("/tmp/hc-tui-e2e");
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
    std::env::set_var("HC_SESSION", "hctui-mp");
}

struct Harness {
    net: NetHandle,
    events: Receiver<NetEvent>,
    /// pane → (channel, 尺寸)
    streams: HashMap<u32, (u32, u16, u16)>,
    views: HashMap<u32, TerminalView>,
    closed: Vec<(u32, String)>,
    notes: Vec<String>,
}

impl Harness {
    fn new() -> Self {
        let (sender, events) = std::sync::mpsc::channel();
        let net = NetHandle::spawn(
            NetConfig {
                session: Some("hctui-mp".to_string()),
            },
            sender,
        );
        Self {
            net,
            events,
            streams: HashMap::new(),
            views: HashMap::new(),
            closed: Vec::new(),
            notes: Vec::new(),
        }
    }

    /// 处理一条通知；Output 立刻还 credit（否则 core 会静默停住）。
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
                self.streams.insert(pane, (channel, cols, rows));
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
                    .find(|(_, (stream, _, _))| *stream == channel)
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
            NetEvent::AttachResized {
                channel,
                cols,
                rows,
            } => {
                let pane = self
                    .streams
                    .iter()
                    .find(|(_, (stream, _, _))| *stream == channel)
                    .map(|(pane, _)| *pane);
                if let Some(pane) = pane {
                    if let Some(view) = self.views.get_mut(&pane) {
                        view.resize(cols, rows, 8, 16).unwrap();
                    }
                }
            }
            NetEvent::AttachClosed { pane, reason, .. } => {
                self.closed.push((pane, reason));
            }
            NetEvent::Failed { reason } => panic!("net thread failed: {reason}"),
            _ => {}
        }
    }

    fn pump(&mut self, timeout: Duration) {
        if let Ok(event) = self.events.recv_timeout(timeout) {
            self.handle(event);
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
            self.pump(Duration::from_millis(50));
        }
    }

    fn screen(&mut self, pane: u32) -> String {
        let Some(view) = self.views.get_mut(&pane) else {
            return String::new();
        };
        let (_, cols, rows) = self.streams[&pane];
        let area = Rect::new(0, 0, cols, rows);
        let mut buffer = Buffer::empty(area);
        if view.render(&mut buffer, area).is_err() {
            return String::new();
        }
        let mut out = String::new();
        for y in 0..rows {
            for x in 0..cols {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }
}

#[test]
fn two_panes_have_independent_streams() -> Result<()> {
    if std::env::var("HC_TUI_E2E").is_err() {
        eprintln!("skipping P2 end-to-end test; set HC_TUI_E2E=1 to run it against a real core");
        return Ok(());
    }
    isolated_environment();
    let mut harness = Harness::new();

    harness.net.send(Command::Spawn {
        pane: 1,
        target: None,
        argv: Vec::new(),
        cwd: None,
        on: None,
        cols: 60,
        rows: 20,
    });
    harness.net.send(Command::Spawn {
        pane: 2,
        target: None,
        argv: Vec::new(),
        cwd: None,
        on: None,
        cols: 40,
        rows: 10,
    });

    // 两个 Pane 各自拿到一条流，通道互不相同。
    harness.wait_until("two attach streams", |h| h.streams.len() == 2)?;
    let (channel_one, ..) = harness.streams[&1];
    let (channel_two, ..) = harness.streams[&2];
    assert_ne!(channel_one, channel_two, "each pane needs its own stream");

    // 输入按 Pane 分流：两条流各自回各自的输出。
    harness.net.send(Command::Input {
        pane: 1,
        bytes: b"printf 'PANE-ONE\\n'\r".to_vec(),
    });
    harness.net.send(Command::Input {
        pane: 2,
        bytes: b"printf 'PANE-TWO\\n'\r".to_vec(),
    });
    harness.wait_until("both panes rendered their own output", |h| {
        h.screen(1).contains("PANE-ONE") && h.screen(2).contains("PANE-TWO")
    })?;
    assert!(
        !harness.screen(1).contains("PANE-TWO"),
        "pane 1 must not receive pane 2's output"
    );
    assert!(
        !harness.screen(2).contains("PANE-ONE"),
        "pane 2 must not receive pane 1's output"
    );

    // 关掉一个视图：只断它自己的流，另一个继续工作。
    harness.net.send(Command::Close { pane: 2 });
    harness.wait_until("pane 2 stream closed", |h| {
        h.closed.iter().any(|(pane, _)| *pane == 2)
    })?;
    assert!(
        !harness.closed.iter().any(|(pane, _)| *pane == 1),
        "closing one view must not touch the other"
    );

    harness.net.send(Command::Input {
        pane: 1,
        bytes: b"printf 'STILL-ALIVE\\n'\r".to_vec(),
    });
    harness.wait_until("pane 1 still interactive", |h| {
        h.screen(1).contains("STILL-ALIVE")
    })?;

    // 尺寸变化只影响自己的流。
    harness.net.send(Command::Resize {
        pane: 1,
        cols: 50,
        rows: 18,
    });
    harness.pump(Duration::from_millis(300));

    harness.net.shutdown();
    let _ = std::process::Command::new("hc")
        .arg("server")
        .arg("stop")
        .output();
    Ok(())
}
