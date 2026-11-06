//! G1 端到端验收：真实 core、隔离 session。
//!
//! 覆盖 plan v1.1 §8 的 G1 门禁里能自动化的部分：
//! 首帧渲染、输入回环、跨 credit 窗口的大流量、detach 不杀进程、程序结束的 CLOSE、
//! 重连后重新 attach 的首帧。
//!
//! 需要已安装 `hc`。默认跳过，用下面这条命令跑：
//!
//! ```sh
//! HC_TUI_E2E=1 cargo test -p hc-tui --test g1_attach -- --nocapture
//! ```
//!
//! 全程使用 `/tmp/hc-tui-e2e` 下的独立 XDG 路径与 `hctui-e2e` session，
//! 不会碰用户正在使用的 core。

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use ratatui::{buffer::Buffer, layout::Rect};
use serde_json::{json, Value};
use tokio::process::Child;

use hc_tui::hc::client::{self, Parts};
use hc_tui::hc::codec;
use hc_tui::term::view::TerminalView;

const COLS: u16 = 80;
const ROWS: u16 = 24;

struct Link {
    parts: Parts,
    next_id: u64,
    events_channel: Option<u32>,
    attach_channel: Option<u32>,
}

impl Link {
    fn new(parts: Parts) -> Self {
        Self {
            parts,
            next_id: 0,
            events_channel: None,
            attach_channel: None,
        }
    }

    fn child(&mut self) -> &mut Child {
        &mut self.parts.child
    }

    async fn write(&mut self, bytes: &[u8]) -> Result<()> {
        client::write_frame(&mut self.parts.stdin, bytes).await
    }

    async fn credit(&mut self, channel: u32, bytes: usize) -> Result<()> {
        self.write(&codec::credit(channel, bytes)?).await
    }

    async fn input(&mut self, bytes: &[u8]) -> Result<()> {
        let channel = self.attach_channel.context("no attach channel")?;
        self.write(&codec::encode(channel, codec::DATA, bytes)?)
            .await
    }

    /// 读一帧并按协议还 credit。attach 字节追加到 `screen`。
    async fn read(&mut self, screen: &mut Vec<u8>) -> Result<codec::Frame> {
        let frame = tokio::time::timeout(
            Duration::from_secs(5),
            client::read_frame(&mut self.parts.stdout),
        )
        .await
        .context("core sent nothing for 5 s")??;
        match frame.kind {
            codec::DATA => {
                if Some(frame.channel) == self.attach_channel {
                    screen.extend_from_slice(&frame.payload);
                    self.credit(frame.channel, frame.payload.len()).await?;
                } else if Some(frame.channel) == self.events_channel || frame.channel != 0 {
                    // 事件流也要还 credit，否则长跑后会静默停住。
                    self.credit(frame.channel, frame.payload.len()).await?;
                }
            }
            codec::CREDIT => {}
            _ => {}
        }
        Ok(frame)
    }

    async fn rpc(&mut self, method: &str, params: Value, screen: &mut Vec<u8>) -> Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        let request = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        self.write(&codec::encode(
            0,
            codec::DATA,
            request.to_string().as_bytes(),
        )?)
        .await?;
        loop {
            let frame = self.read(screen).await?;
            if frame.channel != 0 || frame.kind != codec::DATA {
                continue;
            }
            let Ok(reply) = serde_json::from_slice::<Value>(&frame.payload) else {
                continue;
            };
            if reply.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = reply.get("error") {
                anyhow::bail!("{method} failed: {error}");
            }
            return Ok(reply.get("result").cloned().unwrap_or(Value::Null));
        }
    }
}

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
    std::env::set_var("HC_SESSION", "hctui-e2e");
}

fn stop_core() {
    let _ = std::process::Command::new("hc")
        .arg("server")
        .arg("stop")
        .output();
}

/// 把 VT 内容画进一个 Buffer，再按行拼成可搜索的文本。
fn screen_text(view: &mut TerminalView) -> String {
    let area = Rect::new(0, 0, COLS, ROWS);
    let mut buffer = Buffer::empty(area);
    if view.render(&mut buffer, area).is_err() {
        return String::new();
    }
    let mut out = String::new();
    for y in 0..ROWS {
        for x in 0..COLS {
            out.push_str(buffer[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

async fn wait_for_screen(
    link: &mut Link,
    view: &mut TerminalView,
    screen: &mut Vec<u8>,
    needle: &str,
) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let text = screen_text(view);
        if text.contains(needle) {
            return Ok(());
        }
        if Instant::now() > deadline {
            anyhow::bail!("screen never showed {needle:?}; last screen:\n{text}");
        }
        let frame = link.read(screen).await?;
        if Some(frame.channel) == link.attach_channel && frame.kind == codec::DATA {
            view.write(&frame.payload);
            let responses = view.take_responses();
            if !responses.is_empty() {
                link.input(&responses).await?;
            }
        }
        if Some(frame.channel) == link.attach_channel && frame.kind == codec::CLOSE {
            anyhow::bail!("attach stream closed while waiting for {needle:?}");
        }
    }
}

async fn connect() -> Result<Link> {
    let parts = client::open(Some("hctui-e2e"))
        .await
        .map_err(|error| match error {
            client::OpenError::Fatal(error) | client::OpenError::Retry(error) => error,
        })?;
    assert_eq!(parts.hello.generation, 1);
    assert!(parts.hello.framed);
    Ok(Link::new(parts))
}

async fn attach(link: &mut Link, terminal: &str, screen: &mut Vec<u8>) -> Result<u32> {
    let result = link
        .rpc(
            "terminal.attach",
            json!({
                "terminal": terminal,
                "cols": COLS,
                "rows": ROWS,
                "history": 0,
                "observe": false,
                "display": {"fg": null, "bg": null, "cursor": null, "cell": [8, 16]},
            }),
            screen,
        )
        .await?;
    assert_eq!(
        result.get("resize_notifications"),
        Some(&Value::Bool(false)),
        "client viewers get no core-to-viewer RESIZE"
    );
    let channel = result
        .get("channel")
        .and_then(Value::as_u64)
        .context("attach returned no channel")? as u32;
    link.attach_channel = Some(channel);
    link.write(&codec::facts(
        channel,
        br#"{"fg":null,"bg":null,"cursor":null,"cell":[8,16]}"#,
    )?)
    .await?;
    Ok(channel)
}

/// 读到首帧（attach 流上第一段 DATA）。
async fn wait_for_first_paint(
    link: &mut Link,
    view: &mut TerminalView,
    screen: &mut Vec<u8>,
) -> Result<Vec<u8>> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut paint = Vec::new();
    loop {
        if Instant::now() > deadline {
            anyhow::bail!("no first paint within 10 s");
        }
        let frame = link.read(screen).await?;
        if Some(frame.channel) == link.attach_channel && frame.kind == codec::DATA {
            // 首帧是「整屏复位 + 重放」，core 一次发完一屏，所以第一段 DATA 就是全屏。
            paint.extend_from_slice(&frame.payload);
            view.write(&paint);
            return Ok(paint);
        }
    }
}

#[tokio::test]
async fn g1_single_pane_end_to_end() -> Result<()> {
    if std::env::var("HC_TUI_E2E").is_err() {
        eprintln!("skipping G1 end-to-end test; set HC_TUI_E2E=1 to run it against a real core");
        return Ok(());
    }
    isolated_environment();
    struct CoreGuard;
    impl Drop for CoreGuard {
        fn drop(&mut self) {
            stop_core();
        }
    }
    let _core_guard = CoreGuard;
    stop_core();
    let mut screen = Vec::new();
    let mut link = connect().await?;

    let run = link.rpc("terminal.run", json!({}), &mut screen).await?;
    let address = run
        .get("address")
        .and_then(Value::as_str)
        .context("terminal.run returned no address")?
        .to_string();

    let subscribe = link.rpc("events.subscribe", json!({}), &mut screen).await?;
    let events_channel = subscribe
        .get("channel")
        .and_then(Value::as_u64)
        .context("subscribe returned no channel")? as u32;
    link.events_channel = Some(events_channel);

    let channel = attach(&mut link, &address, &mut screen).await?;
    let mut view = TerminalView::new(COLS, ROWS, 1 << 20)?;
    let paint = wait_for_first_paint(&mut link, &mut view, &mut screen).await?;
    // 首帧可能是空屏（程序还没输出），但必须是一段完整的复位 + 重放序列。
    assert!(!paint.is_empty(), "attach must start with a full paint");
    assert!(
        paint.starts_with(b"\x18\x1b[!p"),
        "first paint must begin with cancel + soft reset"
    );

    // 键盘 → attach DATA → core → 回显 → 本地 VT 渲染。
    link.input(b"printf 'HCTUI-MARK-%s\\n' $((6*7))\r").await?;
    wait_for_screen(&mut link, &mut view, &mut screen, "HCTUI-MARK-42").await?;

    // 跨过 256 KiB credit 窗口：不回 credit 的话 core 会静默停住。
    link.input(b"seq 1 40000\r").await?;
    wait_for_screen(&mut link, &mut view, &mut screen, "40000").await?;

    // detach 只断视图，程序继续跑。
    link.write(&codec::close(channel)?).await?;
    let list = link.rpc("terminal.list", json!({}), &mut screen).await?;
    let alive = list
        .get("terminals")
        .and_then(Value::as_array)
        .map(|terminals| {
            terminals
                .iter()
                .any(|terminal| terminal.get("address").and_then(Value::as_str) == Some(&address))
        })
        .unwrap_or(false);
    assert!(alive, "detach must not kill the program in core");

    // 重连：新连接 → 重新订阅 → 重新 attach → 新首帧。
    link.child().start_kill().ok();
    drop(link);
    let mut second = connect().await?;
    let subscribe = second
        .rpc("events.subscribe", json!({}), &mut screen)
        .await?;
    second.events_channel = subscribe
        .get("channel")
        .and_then(Value::as_u64)
        .map(|c| c as u32);
    attach(&mut second, &address, &mut screen).await?;
    let mut view = TerminalView::new(COLS, ROWS, 1 << 20)?;
    let paint = wait_for_first_paint(&mut second, &mut view, &mut screen).await?;
    assert!(
        !paint.is_empty(),
        "reconnect must be followed by a full paint"
    );
    // 重连后画面里应仍有上一轮的输出。
    wait_for_screen(&mut second, &mut view, &mut screen, "HCTUI-MARK-42").await?;

    // 程序结束：core 先发完输出，再发 CLOSE。
    let run = second
        .rpc(
            "terminal.run",
            json!({"argv": ["sh", "-c", "echo HCTUI-DONE; sleep 1"]}),
            &mut screen,
        )
        .await?;
    let short = run
        .get("address")
        .and_then(Value::as_str)
        .context("run returned no address")?
        .to_string();
    second
        .write(&codec::close(second.attach_channel.unwrap())?)
        .await?;
    attach(&mut second, &short, &mut screen).await?;
    let mut view = TerminalView::new(COLS, ROWS, 1 << 20)?;
    let _ = wait_for_first_paint(&mut second, &mut view, &mut screen).await?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut reason = None;
    while Instant::now() < deadline && reason.is_none() {
        let frame = second.read(&mut screen).await?;
        if Some(frame.channel) == second.attach_channel && frame.kind == codec::DATA {
            view.write(&frame.payload);
        }
        if Some(frame.channel) == second.attach_channel && frame.kind == codec::CLOSE {
            reason = Some(String::from_utf8_lossy(&frame.payload).into_owned());
        }
    }
    assert!(
        reason.is_some(),
        "core must send CLOSE after the program exits"
    );

    if std::process::Command::new("nvim")
        .arg("--version")
        .output()
        .is_ok()
    {
        let path = Path::new("/tmp/hc-tui-e2e/nvim-input.txt");
        std::fs::write(path, "original line\n")?;
        let run = second
            .rpc(
                "terminal.run",
                json!({
                    "argv":["nvim","--clean","-u","NONE","-i","NONE","-n",path],
                    "cols":COLS,"rows":ROWS,
                }),
                &mut screen,
            )
            .await?;
        let address = run["address"]
            .as_str()
            .context("nvim returned no address")?;
        attach(&mut second, address, &mut screen).await?;
        let mut view = TerminalView::new(COLS, ROWS, 1 << 20)?;
        wait_for_first_paint(&mut second, &mut view, &mut screen).await?;
        wait_for_screen(&mut second, &mut view, &mut screen, "original line").await?;
        second.input(b"i").await?;
        let prefix = "HCTUI-NVIM-abc";
        for byte in prefix.bytes() {
            second.input(&[byte]).await?;
        }
        let paste = "中文-input-abc ".repeat(256);
        second.input(&view.encode_paste(&paste)).await?;
        wait_for_screen(&mut second, &mut view, &mut screen, "-input-abc").await?;
        second.input(b"\x1b").await?;
        second
            .write(&codec::resize(second.attach_channel.unwrap(), 62, 19)?)
            .await?;
        view.resize(62, 19, 8, 16)?;
        second.input(b":wq!\r").await?;
        let mut closed = false;
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline && !closed {
            let frame = second.read(&mut screen).await?;
            if Some(frame.channel) == second.attach_channel {
                if frame.kind == codec::DATA {
                    view.write(&frame.payload);
                    let mut buffer = Buffer::empty(Rect::new(0, 0, 62, 19));
                    view.render(&mut buffer, Rect::new(0, 0, 62, 19))?;
                    let response = view.take_responses();
                    if !response.is_empty() {
                        second.input(&response).await?;
                    }
                }
                closed = frame.kind == codec::CLOSE;
            }
        }
        assert!(closed, "nvim must save and exit after resize and paste");
        assert_eq!(
            std::fs::read_to_string(path)?,
            format!("{prefix}{paste}original line\n"),
            "typed ASCII and multilingual bracketed paste must reach nvim exactly once"
        );
        eprintln!("nvim real core: ASCII typing, multilingual paste, resize and save passed");
    }

    stop_core();
    Ok(())
}
