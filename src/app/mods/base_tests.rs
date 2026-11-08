//! base 票的验收用例：新增域、方法与动作全部经真实宿主分发（`host.call`）跑一遍。
//!
//! 模块把每条应答与收到的事件写进通知队列，用例再打印队列内容，作为证据段的
//! 原始输出。运行：`cargo test --lib base_ticket -- --nocapture`

use super::*;
use crate::config::Config;
use crate::mods::{ModNotification, SurfacePlace};
use ratatui_core::backend::TestBackend;
use ratatui_core::terminal::Terminal;
use serde_json::json;

/// 带脚本的探针：按 `props.script` 顺序调用宿主，结束后把应答写进通知。
const PROBE_TSX: &str = r#"
import { host, mount, signal, Text, View } from "nsh";

type Step = { name: string; data?: any; label?: string };
const lines = signal<string[]>([]);

function resolve(value: any, last: any): any {
  if (Array.isArray(value)) return value.map((item) => resolve(item, last));
  if (value && typeof value === "object") {
    if ("$last" in value) return last ? last[(value as any).$last] : undefined;
    if ("$param" in value) return (host.params as any)[(value as any).$param];
    const out: any = {};
    for (const key of Object.keys(value)) out[key] = resolve((value as any)[key], last);
    return out;
  }
  return value;
}

function note(title: string, body: any) {
  host.call("notify", { title, body: typeof body === "string" ? body : JSON.stringify(body) })
    .catch(() => {});
}

async function main() {
  host.on("overlay.result", (payload: any) => note("event:overlay.result", payload));
  host.on("action.finished", (payload: any) => note("event:action.finished", payload));
  host.on("command", (payload: any) => note("event:command", payload));
  host.on("props.changed", (payload: any) => note("event:props.changed", payload));
  const steps = ((host.params as any).props?.script ?? []) as Step[];
  const out: string[] = [];
  let last: any = null;
  for (const step of steps) {
    const label = step.label ?? step.name;
    try {
      const data = resolve(step.data ?? {}, last);
      const reply = await host.call(step.name as any, data as any);
      last = reply;
      out.push(label + " => " + JSON.stringify(reply));
    } catch (err) {
      out.push(label + " !! " + (err as Error).message);
    }
    lines.set(out.slice());
  }
  await host.call("notify", { title: "report", body: out.join("\n") });
}

main().catch((err: Error) => note("fatal", err.message));
mount(() => <View><Text>{lines().join("\n")}</Text></View>);
"#;

/// 被 `ask` 调用的模块：收到 props 就 `answer`。
const ANSWER_TSX: &str = r#"
import { host, mount, signal, Text, View } from "nsh";

const seen = signal("waiting");
host.on("props.changed", (payload: any) => {
  const ask = payload?.props?.ask;
  seen.set(String(ask));
  host.call("answer", { value: { picked: ask } }).catch((err: Error) => seen.set(err.message));
});
mount(() => <View><Text>{seen()}</Text></View>);
"#;

fn app() -> App {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    let dir = std::env::temp_dir().join(format!("hctui-base-{}-{unique}", std::process::id()));
    let (_, events) = std::sync::mpsc::channel();
    App::with_network(
        Config {
            restore: false,
            prompt_new_tab: false,
            config_path: dir.join("config.toml"),
            state_path: dir.join("state.json"),
            ..Config::default()
        },
        None,
        events,
    )
    .unwrap()
}

/// 写一个本地模块：dist 里的 bundle 与 `hctui.toml` 里的授予。
fn write_mod(app: &App, id: &str, tsx: &str, host: &[&str], events: &[&str]) {
    let root = app.config.mods_dir().join(id);
    std::fs::create_dir_all(root.join("dist")).unwrap();
    let code = nsh_bundler::compile_module("App.tsx", tsx, &[]).unwrap();
    std::fs::write(root.join("dist").join("bundle.js"), code).unwrap();
    let capabilities = json!({"host": host});
    std::fs::write(
        root.join("dist").join("capabilities.json"),
        capabilities.to_string(),
    )
    .unwrap();
    let list = |items: &[&str]| {
        items
            .iter()
            .map(|name| format!("\"{name}\""))
            .collect::<Vec<_>>()
            .join(", ")
    };
    std::fs::write(
        root.join("hctui.toml"),
        format!(
            "[mod]\nid = \"{id}\"\nhost_api = 1\n\n[grants]\nhost = [{}]\nevents = [{}]\n",
            list(host),
            list(events)
        ),
    )
    .unwrap();
}

fn pump(app: &mut App, count: usize) {
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    for _ in 0..count {
        terminal.draw(|frame| crate::ui::draw(frame, app)).unwrap();
        app.pump_mods();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn print_notes(app: &App) {
    for item in &app.mod_notifications {
        println!("--- {} [{}] ---\n{}", item.id, item.level, item.text);
    }
}

#[test]
fn base_ticket_live_calls() {
    let mut app = app();
    // 夹具：一个终端、一份机器表、一份模板缓存、一条待撤通知。
    let pane = app.focused();
    if let Some(state) = app.panes.get_mut(&pane) {
        state.terminal = Some("box/t7".to_string());
    }
    app.workspaces[0].label = "api".into();
    app.machines = vec![crate::app::MachineInfo {
        name: "box".into(),
        target: "box:22".into(),
        state: "connected".into(),
        host: "box".into(),
    }];
    app.mod_templates = vec![json!({"name":"dev","title":"Dev","description":"dev shell"})];
    app.mod_notifications.push(ModNotification {
        id: "seed".into(),
        level: "warn".into(),
        text: "queued before the probe".into(),
        action: None,
        target: None,
        surface: None,
        created: 1,
        created_ms: 1_000,
        timeout_ms: 0,
        since: std::time::Instant::now(),
    });
    // 第二个 Space：用来验证 `space` 过滤。
    app.new_workspace(Some("second".into()));
    let second = app.focused();
    if let Some(state) = app.panes.get_mut(&second) {
        state.terminal = Some("box/t9".to_string());
    }
    app.active_ws = 0;
    std::fs::create_dir_all(app.config.mods_dir()).unwrap();
    let tmp = app.config.state_path.parent().unwrap().join("upload.txt");
    std::fs::write(&tmp, b"hello").unwrap();

    // 各域的体积量级：条数与序列化字节数，写进 docs/mods.md 的表。
    let all = app.mod_state_snapshot(&[]);
    for domain in [
        "machines",
        "machineInfo",
        "spaces",
        "tabs",
        "panes",
        "agents",
        "terminals",
        "focused",
        "theme",
        "commands",
        "keybindings",
        "settings",
        "templates",
        "notifications",
    ] {
        let value = all.get(domain).cloned().unwrap_or(serde_json::Value::Null);
        let bytes = value.to_string().len();
        let count = match &value {
            serde_json::Value::Array(list) => list.len(),
            serde_json::Value::Object(map) => map.len(),
            _ => 0,
        };
        println!("domain {domain}: {count} 条 / {bytes} 字节");
    }

    let events = [
        "state.changed",
        "props.changed",
        "overlay.result",
        "command",
        "action.finished",
    ];
    write_mod(&app, "answerer", ANSWER_TSX, &["answer", "notify"], &events);
    write_mod(
        &app,
        "probe",
        PROBE_TSX,
        &["state", "setProps", "notify", "dismiss", "ask", "run"],
        &events,
    );

    let answerer = app
        .mods_open(
            "answerer",
            SurfacePlace::Dialog,
            json!({}),
            Some((40, 8)),
            None,
            None,
        )
        .unwrap();
    let script = json!([
        {"name": "state", "label": "state.domains", "data": {"domains": [
            "commands", "settings", "keybindings", "machineInfo", "templates",
            "notifications", "panes", "theme"
        ]}},
        {"name": "state", "label": "state.space", "data": {
            "domains": ["spaces", "tabs", "panes", "agents", "terminals"], "space": "sp1"
        }},
        {"name": "setProps", "label": "setProps", "data": {
            "surface": {"$param": "surface"}, "props": {"ping": 1}
        }},
        {"name": "ask", "label": "ask", "data": {
            "surface": answerer, "props": {"ask": "pick-one"}, "timeout_ms": 2000
        }},
        {"name": "run", "label": "run terminal.send", "data": {
            "name": "terminal.send", "args": {"terminal": "box/t7", "text": "echo hi"}
        }},
        {"name": "run", "label": "run terminal.keys", "data": {
            "name": "terminal.keys", "args": {"terminal": "box/t7", "keys": ["C-c", "Enter"]}
        }},
        {"name": "run", "label": "run terminal.wait", "data": {
            "name": "terminal.wait", "args": {"terminal": "box/t7", "output": "ready"}
        }},
        {"name": "run", "label": "run terminal.put", "data": {
            "name": "terminal.put", "args": {"terminal": "box/t7", "path": tmp.to_string_lossy()}
        }},
        {"name": "run", "label": "run machine.rename", "data": {
            "name": "machine.rename", "args": {"machine": "box", "name": "renamed"}
        }},
        {"name": "run", "label": "run space.close", "data": {"name": "space.close", "args": {}}},
        {"name": "notify", "label": "notify", "data": {
            "title": "note-a", "body": "needs you", "level": "warn",
            "action": {"name": "agent.next"}, "target": "pane:1", "surface": "probe"
        }},
        {"name": "dismiss", "label": "dismiss", "data": {"id": {"$last": "id"}}},
    ]);
    let probe = app
        .mods_open(
            "probe",
            SurfacePlace::Dialog,
            json!({"script": script}),
            Some((60, 16)),
            None,
            None,
        )
        .unwrap();
    println!("answerer surface = {answerer}, probe surface = {probe}");
    pump(&mut app, 300);

    println!(
        "probe props after setProps = {}",
        app.mods
            .surface(&probe)
            .and_then(|surface| surface.props.get("ping").cloned())
            .unwrap_or(serde_json::Value::Null)
    );

    // 确认态：space.close 打开了确认，Esc 收敛到 cancelled。
    println!(
        "prompt open = {}, requests = {}",
        app.prompt.is_some(),
        serde_json::to_string(
            &app.mods
                .requests
                .values()
                .map(|record| (record.action.clone(), record.status.clone()))
                .collect::<Vec<_>>()
        )
        .unwrap()
    );
    app.on_key(crossterm::event::KeyEvent::new(
        crossterm::event::KeyCode::Esc,
        crossterm::event::KeyModifiers::NONE,
    ));
    pump(&mut app, 40);

    // command 事件：模块命令 `<mod>.<command>` 经宿主投递。
    app.execute_action("probe.say", json!({"text": "hi"}))
        .unwrap();
    pump(&mut app, 40);

    println!("inflight RPCs:");
    for (tag, (method, params)) in &app.inflight {
        let mut shown = params.clone();
        if let Some(data) = shown.get_mut("data") {
            let size = data.as_str().map(str::len).unwrap_or(0);
            *data = json!(format!("<{size} bytes base64>"));
        }
        println!("  {tag} {method} {shown}");
    }
    // 异步终态：core 应答到达后 action.finished 才收尾。
    app.on_reply("send", Ok(json!({"turn": 3})));
    pump(&mut app, 30);
    app.on_reply(
        "wait",
        Err(crate::hc::net::RpcFailure {
            kind: "unavailable".into(),
            message: "machine unreachable".into(),
        }),
    );
    pump(&mut app, 30);
    println!(
        "requests after replies = {}",
        serde_json::to_string(
            &app.mods
                .requests
                .values()
                .map(|record| (
                    record.action.clone(),
                    record.status.clone(),
                    record.error.clone()
                ))
                .collect::<Vec<_>>()
        )
        .unwrap()
    );
    println!(
        "notification ids = {:?}",
        app.mod_notifications
            .iter()
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>()
    );
    print_notes(&app);

    // 断言：报告与各事件都在队列里。
    let texts: Vec<(String, String)> = app
        .mod_notifications
        .iter()
        .map(|item| (item.id.clone(), item.text.clone()))
        .collect();
    for needle in [
        "state.domains =>",
        "state.space =>",
        "setProps =>",
        "ask =>",
        "run terminal.send =>",
        "run terminal.keys =>",
        "run terminal.wait =>",
        "run terminal.put =>",
        "run machine.rename =>",
        "notify =>",
        "dismiss =>",
        "event:props.changed",
        "event:overlay.result",
        "event:action.finished",
        "event:command",
    ] {
        assert!(
            texts.iter().any(|(_, text)| text.contains(needle)),
            "missing {needle}"
        );
    }
    let report = texts
        .iter()
        .find(|(_, text)| text.contains("ask =>"))
        .expect("report present")
        .1
        .clone();
    assert!(report.contains("pick-one"), "ask reply: {report}");
    assert!(
        report.contains("confirmation_pending\":true"),
        "confirm: {report}"
    );
    assert!(app.prompt.is_none());
    assert!(!app
        .mod_notifications
        .iter()
        .any(|item| item.text.contains("note-a")));
    assert!(texts
        .iter()
        .any(|(_, text)| text.contains("\"status\":\"done\"")));
    assert!(texts
        .iter()
        .any(|(_, text)| text.contains("\"status\":\"failed\"") && text.contains("unreachable")));
    assert!(app
        .mods
        .requests
        .values()
        .any(|record| record.action == "space.close" && record.status == "cancelled"));
}

/// 现有示例模块与 git-board 包：不改源码、默认参数，仍能加载并渲染。
#[test]
fn examples_still_load_with_defaults() {
    let mut app = app();
    std::fs::create_dir_all(app.config.mods_dir()).unwrap();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let cases: [(&str, &[&str], &[&str], SurfacePlace); 3] = [
        (
            "agents",
            &["state", "run", "focus", "notify"],
            &["state.changed", "focus.changed", "app.ready"],
            SurfacePlace::Region,
        ),
        (
            "spaces",
            &["state", "run", "focus"],
            &["state.changed", "focus.changed", "app.ready"],
            SurfacePlace::Region,
        ),
        (
            "prs",
            &["state", "exec", "overlay", "closeOverlay"],
            &["app.ready", "theme.changed"],
            SurfacePlace::Dialog,
        ),
    ];
    let mut opened = Vec::new();
    for (id, host, events, place) in cases {
        let source = root.join("examples").join("mods").join(id);
        let tsx = std::fs::read_to_string(source.join("App.tsx")).unwrap();
        write_mod(&app, id, &tsx, host, events);
        opened.push(
            app.mods_open(id, place, json!({}), Some((40, 12)), None, None)
                .unwrap(),
        );
    }
    // git-board 包：一份清单同时给动作、终端入口与模块，模块走包记录加载。
    let package = root.join("examples").join("packages").join("git-board");
    crate::mods::store::link(&app.config, &package).unwrap();
    opened.push(
        app.mods_open(
            "dev.hctui.pkg.git-board",
            SurfacePlace::Region,
            json!({}),
            Some((40, 12)),
            None,
            None,
        )
        .unwrap(),
    );
    pump(&mut app, 80);
    for id in &opened {
        let surface = app.mods.surface(id).expect("surface stays");
        let errors = surface
            .errors
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        println!(
            "example surface {} mod={} state={:?} engine errors={:?}",
            surface.id, surface.mod_id, surface.state, errors
        );
        assert_eq!(surface.state, crate::mods::SurfaceState::Running);
        assert!(errors.is_empty(), "{}: {errors:?}", surface.mod_id);
    }
}
