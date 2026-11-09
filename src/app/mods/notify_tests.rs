//! notifycli 票的用例：命令行 `notify` 的控制通道、级别、动作与目标。
//!
//! 运行：`cargo test --lib notify_cli -- --nocapture`

use super::toast_tests::wait_until;
use super::*;
use crate::config::Config;
use crate::mods::SurfacePlace;
use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui_core::backend::TestBackend;
use ratatui_core::terminal::Terminal;
use serde_json::json;

/// 提示表面模块：只把队列条数画出来，够证明 props 到了表面。
const TOAST_TSX: &str = r#"
import { host, mount, signal, Text, View } from "nsh";

const toasts = signal<any[]>([]);

host.on("props.changed", (payload: any) => {
  toasts.set(payload?.props?.toasts ?? []);
});

function App() {
  return <View><Text>{"toasts=" + toasts().length}</Text></View>;
}

mount(App);
"#;

fn app() -> App {
    let dir = crate::testutil::unique_dir("hctui-notify", "app");
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

fn write_mod(app: &App, id: &str) {
    let root = app.config.mods_dir().join(id);
    std::fs::create_dir_all(root.join("dist")).unwrap();
    let code = nsh_bundler::compile_module("App.tsx", TOAST_TSX, &[]).unwrap();
    std::fs::write(root.join("dist").join("bundle.js"), code).unwrap();
    std::fs::write(
        root.join("dist").join("capabilities.json"),
        json!({"host": ["surface"]}).to_string(),
    )
    .unwrap();
    std::fs::write(
        root.join("hctui.toml"),
        format!(
            "[mod]\nid = \"{id}\"\nhost_api = 1\n\n[grants]\nhost = [\"surface\"]\nevents = [\"props.changed\"]\n"
        ),
    )
    .unwrap();
}

fn pump(app: &mut App, count: usize) {
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
    for _ in 0..count {
        terminal.draw(|frame| crate::ui::draw(frame, app)).unwrap();
        app.poll_plugin_registry();
        app.tick();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn click(app: &mut App) {
    let hit = app.toast_hit;
    assert!(hit.width > 0 && hit.height > 0, "toast 没有画出来");
    app.on_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: hit.x + hit.width / 2,
        row: hit.y + 1,
        modifiers: KeyModifiers::NONE,
    });
}

#[test]
fn notify_cli_falls_back_to_builtin_toast_with_level() {
    let mut app = app();
    let reply = app.control_reply(&json!({"method":"notify","text":"build done","level":"error"}));
    println!("reply = {reply}");
    println!(
        "toast = {:?} queue = {}",
        app.toast.as_ref().map(|toast| (&toast.text, &toast.level)),
        app.mod_notifications.len()
    );
    assert_eq!(reply["fallback"], json!("builtin"));
    let toast = app.toast.as_ref().expect("内置 toast");
    assert_eq!(toast.text, "build done");
    assert_eq!(toast.level, "error");

    // 画一帧拿到命中矩形，点一下：没有 action 也没有 target，只清掉 toast。
    pump(&mut app, 1);
    click(&mut app);
    println!("after click toast = {:?}", app.toast);
    assert!(app.toast.is_none());
}

#[test]
fn notify_cli_rejects_bad_input() {
    let mut app = app();
    let empty = app.control_reply(&json!({"method":"notify","text":"  "}));
    let level = app.control_reply(&json!({"method":"notify","text":"x","level":"fatal"}));
    println!("empty = {empty}");
    println!("level = {level}");
    assert_eq!(empty["error"], json!("notify requires a text"));
    assert_eq!(level["error"], json!("level must be info, warn or error"));
}

#[test]
fn notify_cli_click_runs_action() {
    let mut app = app();
    let reply = app.control_reply(&json!({
        "method": "notify",
        "text": "click me",
        "level": "warn",
        "action": {"name": "app.help"},
    }));
    println!("reply = {reply}");
    pump(&mut app, 1);
    click(&mut app);
    println!(
        "after click help = {} note = {:?} toast = {:?}",
        app.help.is_some(),
        app.note,
        app.toast
    );
    assert!(app.help.is_some(), "点击应执行 app.help");
    assert!(app.note.contains("app.help"), "状态行应记录动作");
}

#[test]
fn notify_cli_click_focuses_target_without_action() {
    let mut app = app();
    let reply = app.control_reply(&json!({
        "method": "notify",
        "text": "jump",
        "target": "t1",
    }));
    println!("reply = {reply}");
    pump(&mut app, 1);
    click(&mut app);
    println!("after click note = {:?}", app.note);
    assert!(app.note.contains("agent.focus"), "状态行应记录 agent.focus");
}

#[test]
fn notify_cli_routes_to_toast_surface() {
    let mut app = app();
    std::fs::create_dir_all(app.config.mods_dir()).unwrap();
    write_mod(&app, "toast");
    app.mods_open("toast", SurfacePlace::Toast, json!({}), None, None, None)
        .unwrap();
    pump(&mut app, 20);
    let reply = app.control_reply(&json!({
        "method": "notify",
        "text": "to surface",
        "level": "warn",
        "action": {"name": "app.help"},
        "target": "t1",
    }));
    pump(&mut app, 20);
    // 通知 4 秒后过期：按条件等，不按帧数等。
    assert!(
        wait_until(&mut app, 3.0, |app| app
            .mods
            .surfaces
            .iter()
            .find(|surface| surface.place == SurfacePlace::Toast)
            .is_some_and(|surface| surface.props["toasts"]
                .as_array()
                .is_some_and(|list| list.len() == 1))),
        "通知没有推到提示表面"
    );
    let props = app
        .mods
        .surfaces
        .iter()
        .find(|surface| surface.place == SurfacePlace::Toast)
        .map(|surface| surface.props.clone())
        .expect("toast 表面");
    println!("reply = {reply}");
    println!("props = {props}");
    assert_eq!(reply["fallback"], json!("toast"));
    let toasts = props["toasts"].as_array().expect("toasts 数组");
    assert_eq!(toasts.len(), 1);
    assert_eq!(toasts[0]["text"], json!("to surface"));
    assert_eq!(toasts[0]["level"], json!("warn"));
    assert_eq!(toasts[0]["target"], json!("t1"));
    assert_eq!(toasts[0]["action"], json!({"name": "app.help"}));
}
