//! base2 票的验收用例：提示表面、队列 props、按 id 撤销、超时回收与空域广播。
//!
//! 用真实模块走 `host.call` 与 `props.changed`；打印的原始输出进票据证据段。
//! 运行：`cargo test --lib toast_ticket -- --nocapture`

use super::*;
use crate::config::Config;
use crate::mods::SurfacePlace;
use ratatui_core::backend::TestBackend;
use ratatui_core::terminal::Terminal;
use serde_json::json;

/// 提示表面模块：把宿主推送的队列渲染出来，并回报自己的 place。
const TOAST_TSX: &str = r#"
import { host, mount, signal, Text, View } from "nsh";

const toasts = signal<any[]>([]);
const place = signal("?");

host.on("props.changed", (payload: any) => {
  toasts.set(payload?.props?.toasts ?? []);
});

host.call("surface", {})
  .then((surface: any) => place.set(String(surface?.place)))
  .catch((err: Error) => place.set("surface error: " + err.message));

function App() {
  return (
    <View>
      <Each of={toasts()}>
        {(item: () => any) => (
          <Text>{item().id + " " + item().level + " " + item().text}</Text>
        )}
      </Each>
      <Text>{"place=" + place() + " max=" + toasts().length}</Text>
    </View>
  );
}

mount(App);
"#;

/// 调用方模块：按 `props.script` 顺序调用宿主方法，把应答写进画面。
const CALLER_TSX: &str = r#"
import { host, mount, signal, Text, View } from "nsh";

const out: string[] = [];
const lines = signal<string[]>([]);

async function run(steps: any[]) {
  for (const step of steps) {
    const label = step.label ?? step.name;
    try {
      const reply = await host.call(step.name, step.data ?? {});
      out.push(label + " => " + JSON.stringify(reply));
    } catch (err) {
      out.push(label + " !! " + (err as Error).message);
    }
  }
  lines.set(out.slice());
}

host.on("props.changed", (payload: any) => {
  run(payload?.props?.script ?? []);
});

run((host.params as any)?.props?.script ?? []);

function App() {
  return <View><Text>{lines().join("\n")}</Text></View>;
}

mount(App);
"#;

/// state.changed 探针：最多回报三次事件，避免自己的通知再触发自己的回报。
const WATCH_TSX: &str = r#"
import { host, mount, signal, Text, View } from "nsh";

const seen = signal<string[]>([]);
const lines: string[] = [];
let reports = 0;

host.on("state.changed", (payload: any) => {
  const domains = payload?.domains ?? [];
  // 自己 notify 引起的 notifications 域不再回报，避免自激。
  if (domains.includes("notifications")) return;
  const text = JSON.stringify(domains);
  seen.set(seen().concat([text]));
  if (reports >= 5) return;
  reports += 1;
  // 超时要够长：慢机器上 30 帧可能跨过几秒，条目不能因过期而凭空少掉。
  host.call("notify", { title: "state.changed", body: text, timeout: 600 }).catch(() => {});
});

function App() {
  return (
    <View>
      {lines.map((line) => <Text>{line}</Text>)}
      <Text>{seen().join(" ")}</Text>
    </View>
  );
}

mount(App);
"#;

fn app() -> App {
    let dir = crate::testutil::unique_dir("hctui-toast", "app");
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
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
    for _ in 0..count {
        terminal.draw(|frame| crate::ui::draw(frame, app)).unwrap();
        app.poll_plugin_registry();
        app.tick();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// 出帧到条件成立或 `secs` 秒超时；返回期限内是否达成。
/// 时间轴上只信时钟：慢机器上一帧可能就跨过一整秒，固定帧数算不上等待。
pub(super) fn wait_until(app: &mut App, secs: f32, mut ok: impl FnMut(&App) -> bool) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs_f32(secs);
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
    loop {
        terminal.draw(|frame| crate::ui::draw(frame, app)).unwrap();
        app.poll_plugin_registry();
        app.tick();
        if ok(app) {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// 宿主推送队列的同一条路径（`setProps` 处理器就是这两步）。
fn set_props(app: &mut App, surface: &str, props: serde_json::Value) {
    let surface = app.mods.surface_mut(surface).expect("surface exists");
    surface.props = props.clone();
    surface.host.send("props.changed", json!({"props": props}));
}

fn toast_props(app: &App, surface: &str) -> serde_json::Value {
    app.mods
        .surface(surface)
        .map(|surface| surface.props.clone())
        .unwrap_or(serde_json::Value::Null)
}

/// 提示表面收到的队列长度。
fn toasts_len(app: &App, surface: &str) -> Option<usize> {
    toast_props(app, surface)["toasts"].as_array().map(Vec::len)
}

/// 提示表面实际画出来的行。
fn toast_lines(app: &mut App) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
    terminal.draw(|frame| crate::ui::draw(frame, app)).unwrap();
    let Some(area) = app.mod_toasts else {
        return Vec::new();
    };
    let buffer = terminal.backend().buffer();
    (area.y..area.bottom())
        .map(|y| {
            (area.x..area.right())
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .filter(|line| !line.is_empty())
        .collect()
}

#[test]
fn toast_ticket_queue_dismiss_and_timeout() {
    let mut app = app();
    std::fs::create_dir_all(app.config.mods_dir()).unwrap();
    write_mod(&app, "toast", TOAST_TSX, &["surface"], &["props.changed"]);
    write_mod(&app, "caller", CALLER_TSX, &["notify", "dismiss"], &[]);
    let toast = app
        .mods_open("toast", SurfacePlace::Toast, json!({}), None, None, None)
        .unwrap();
    let script = json!([
        {"name": "notify", "label": "notify-1", "data": {
            "title": "same", "body": "", "level": "warn", "timeout": 30
        }},
        {"name": "notify", "label": "notify-2", "data": {
            "title": "same", "body": "", "level": "info", "timeout": 30
        }},
    ]);
    let caller = app
        .mods_open(
            "caller",
            SurfacePlace::Popup,
            json!({"script": script}),
            Some((60, 10)),
            Some((100, 24)),
            None,
        )
        .unwrap();
    assert!(
        wait_until(&mut app, 3.0, |app| {
            app.mod_notifications.len() == 2 && toasts_len(app, &toast) == Some(2)
        }),
        "两条同文本通知没有都进队列并推到提示表面"
    );
    println!("toast surface = {toast}, caller surface = {caller}");
    println!("focused mod after open = {:?}", app.focused_mod);
    let rect = app.mods.surface(&toast).map(|surface| surface.area);
    println!("toast rect = {rect:?}, content = {:?}", app.content);
    println!(
        "queue = {}",
        serde_json::to_string(
            &app.mod_notifications
                .iter()
                .map(|item| (&item.id, &item.text, item.timeout_ms))
                .collect::<Vec<_>>()
        )
        .unwrap()
    );
    println!("toast props = {}", toast_props(&app, &toast));
    for line in toast_lines(&mut app).iter().take(6) {
        println!("frame | {line}");
    }
    let ids: Vec<String> = app
        .mod_notifications
        .iter()
        .map(|item| item.id.clone())
        .collect();
    assert_eq!(ids.len(), 2, "两条同文本通知都在队列里");
    let entries = &toast_props(&app, &toast)["toasts"];
    assert_eq!(entries.as_array().map(Vec::len), Some(2));
    assert!(entries
        .as_array()
        .unwrap()
        .iter()
        .all(|entry| entry["text"] == json!("same")));

    // 撤掉第一条：只剩另一条同文本通知。
    set_props(
        &mut app,
        &caller,
        json!({"script": [{"name": "dismiss", "data": {"id": ids[0]}}]}),
    );
    assert!(
        wait_until(&mut app, 3.0, |app| {
            app.mod_notifications.len() == 1 && toasts_len(app, &toast) == Some(1)
        }),
        "撤掉一条后队列与提示表面都应剩一条"
    );
    println!(
        "after dismiss {}: queue = {}",
        ids[0],
        serde_json::to_string(
            &app.mod_notifications
                .iter()
                .map(|item| (&item.id, &item.text))
                .collect::<Vec<_>>()
        )
        .unwrap()
    );
    println!("toast props = {}", toast_props(&app, &toast));
    let entries = toast_props(&app, &toast)["toasts"].clone();
    assert_eq!(entries.as_array().map(Vec::len), Some(1));
    assert_eq!(entries[0]["id"], json!(ids[1]));

    // 撤掉第二条：队列与 props 都空。
    set_props(
        &mut app,
        &caller,
        json!({"script": [{"name": "dismiss", "data": {"id": ids[1]}}]}),
    );
    assert!(
        wait_until(&mut app, 3.0, |app| {
            app.mod_notifications.is_empty() && toasts_len(app, &toast) == Some(0)
        }),
        "撤掉最后一条后队列与提示表面都应空"
    );
    println!(
        "after dismiss {}: queue = {} props = {}",
        ids[1],
        app.mod_notifications.len(),
        toast_props(&app, &toast)
    );
    assert!(app.mod_notifications.is_empty());
    assert_eq!(
        toast_props(&app, &toast)["toasts"].as_array().map(Vec::len),
        Some(0)
    );

    // 超时：宿主计时 1 秒后撤掉。
    set_props(
        &mut app,
        &caller,
        json!({"script": [{"name": "notify", "data": {
            "title": "expiring", "body": "", "timeout": 1
        }}]}),
    );
    assert!(
        wait_until(&mut app, 3.0, |app| {
            app.mod_notifications.len() == 1 && toasts_len(app, &toast) == Some(1)
        }),
        "timeout=1 的通知应先入队并推到提示表面"
    );
    println!(
        "after notify timeout=1: props = {}",
        toast_props(&app, &toast)
    );
    assert_eq!(
        toast_props(&app, &toast)["toasts"].as_array().map(Vec::len),
        Some(1)
    );
    // 到期不靠真实等满 1 秒：把宿主时钟往前拨 1.5 秒，`tick` 按拨后的时刻判定。
    app.advance_clock(std::time::Duration::from_millis(1500));
    assert!(
        wait_until(&mut app, 20.0, |app| {
            app.mod_notifications.is_empty() && toasts_len(app, &toast) == Some(0)
        }),
        "时钟拨过 1 秒超时后队列与提示表面都应清空"
    );
    println!(
        "after timeout: queue = {} props = {}",
        app.mod_notifications.len(),
        toast_props(&app, &toast)
    );
    assert!(app.mod_notifications.is_empty());
    assert_eq!(
        toast_props(&app, &toast)["toasts"].as_array().map(Vec::len),
        Some(0)
    );
}

#[test]
fn toast_ticket_surface_shape() {
    let mut app = app();
    std::fs::create_dir_all(app.config.mods_dir()).unwrap();
    write_mod(&app, "toast", TOAST_TSX, &["surface"], &["props.changed"]);
    let toast = app
        .mods_open("toast", SurfacePlace::Toast, json!({}), None, None, None)
        .unwrap();
    pump(&mut app, 30);
    // 打开提示表面不抢焦点，`focus` 也拒绝它。
    app.focus_mod_surface(Some(toast.clone()));
    println!(
        "toast open: focused_mod = {:?}, area = {:?}",
        app.focused_mod,
        app.mods.surface(&toast).map(|surface| surface.area)
    );
    assert!(app.focused_mod.is_none());
    // 唯一表面是提示层：`Esc` 不关它。
    println!("overlay closed by esc = {}", app.close_top_mod_overlay());
    assert!(!app.close_top_mod_overlay());
    // 打开对话框后 `Esc` 只关对话框，提示表面仍在。
    let dialog = app
        .mods_open(
            "toast",
            SurfacePlace::Dialog,
            json!({}),
            Some((50, 8)),
            None,
            None,
        )
        .unwrap();
    pump(&mut app, 30);
    println!("after dialog open: focused_mod = {:?}", app.focused_mod);
    assert_eq!(app.focused_mod.as_deref(), Some(dialog.as_str()));
    println!("overlay closed by esc = {}", app.close_top_mod_overlay());
    println!(
        "toast alive = {}, dialog alive = {}",
        app.mods.surface(&toast).is_some(),
        app.mods.surface(&dialog).is_some()
    );
    assert!(app.mods.surface(&toast).is_some());
    assert!(app.mods.surface(&dialog).is_none());
}

#[test]
fn toast_ticket_builtin_fallback() {
    // 没有提示表面：内置 toast 行为不变，撤销按 id 只撤对应条目。
    let mut app = app();
    let first = app.push_mod_notification("same", "", "info", 4, None, None, None, false);
    let second = app.push_mod_notification("same", "", "warn", 4, None, None, None, false);
    println!(
        "builtin toast = {:?}, ids = [{}, {}]",
        app.toast.as_ref().map(|toast| toast.text.clone()),
        first,
        second
    );
    assert_eq!(
        app.toast.as_ref().map(|toast| toast.text.as_str()),
        Some("same")
    );
    assert_eq!(app.toast_notification.as_deref(), Some(second.as_str()));
    // 撤掉第一条（同文本）：内置 toast 仍在，因为槽位记的是第二条的 id。
    app.dismiss_notification(&first);
    println!(
        "after dismiss {}: toast = {:?}, queue = {}",
        first,
        app.toast.as_ref().map(|toast| toast.text.clone()),
        app.mod_notifications.len()
    );
    assert!(app.toast.is_some());
    app.dismiss_notification(&second);
    println!(
        "after dismiss {}: toast = {:?}, queue = {}",
        second,
        app.toast.as_ref().map(|toast| toast.text.clone()),
        app.mod_notifications.len()
    );
    assert!(app.toast.is_none());
    assert!(app.mod_notifications.is_empty());

    // `--no-mods` 下同一路径照旧。
    let dir = crate::testutil::unique_dir("hctui-toast-nomods", "app");
    let (_, events) = std::sync::mpsc::channel();
    let mut plain = App::with_network(
        Config {
            restore: false,
            prompt_new_tab: false,
            no_mods: true,
            config_path: dir.join("config.toml"),
            state_path: dir.join("state.json"),
            ..Config::default()
        },
        None,
        events,
    )
    .unwrap();
    plain.push_mod_notification("plain", "", "info", 4, None, None, None, false);
    println!(
        "--no-mods toast = {:?}, toast surfaces = {}",
        plain.toast.as_ref().map(|toast| toast.text.clone()),
        plain.toast_surfaces().len()
    );
    assert_eq!(
        plain.toast.as_ref().map(|toast| toast.text.as_str()),
        Some("plain")
    );
    assert!(plain.toast_surfaces().is_empty());
}

#[test]
fn toast_ticket_empty_domains_are_not_broadcast() {
    let mut app = app();
    std::fs::create_dir_all(app.config.mods_dir()).unwrap();
    write_mod(&app, "watch", WATCH_TSX, &["notify"], &["state.changed"]);
    app.mods_open(
        "watch",
        SurfacePlace::Popup,
        json!({}),
        Some((60, 8)),
        Some((100, 24)),
        None,
    )
    .unwrap();
    // 探针的回报是异步的：先等第一条落地，再等 500 毫秒内条数不再变化才记基线。
    assert!(
        wait_until(&mut app, 3.0, |app| !app.mod_notifications.is_empty()),
        "探针没报任何 state.changed"
    );
    let mut before = app.mod_notifications.len();
    let mut quiet = std::time::Instant::now();
    while quiet.elapsed() < std::time::Duration::from_millis(500) {
        pump(&mut app, 1);
        if app.mod_notifications.len() != before {
            before = app.mod_notifications.len();
            quiet = std::time::Instant::now();
        }
    }
    let revision = app.mod_revision;
    app.bump_mod_revision_domains(&[]);
    assert!(
        wait_until(&mut app, 3.0, |app| app.mod_revision > revision),
        "空域 bump 没推进 revision"
    );
    // 空域只推进 revision，不广播：给探针 500 毫秒反应时间，条数一直不变。
    let observe = std::time::Instant::now() + std::time::Duration::from_millis(500);
    while std::time::Instant::now() < observe {
        pump(&mut app, 1);
        assert_eq!(app.mod_notifications.len(), before, "空域 bump 广播了通知");
    }
    println!(
        "empty bump: revision {} -> {}, notifications {} -> {}",
        revision,
        app.mod_revision,
        before,
        app.mod_notifications.len()
    );
    // 真实域照常广播。
    app.bump_mod_revision_domains(&["theme"]);
    assert!(
        wait_until(&mut app, 3.0, |app| app
            .mod_notifications
            .iter()
            .any(|item| item.text.contains("[\"theme\"]"))),
        "主题域没有广播"
    );
    let texts: Vec<String> = app
        .mod_notifications
        .iter()
        .map(|item| item.text.clone())
        .collect();
    println!("theme bump: notifications = {texts:?}");
    assert!(texts.iter().any(|text| text.contains("[\"theme\"]")));
}

#[test]
fn toast_ticket_notify_routing() {
    let mut app = app();
    std::fs::create_dir_all(app.config.mods_dir()).unwrap();
    write_mod(&app, "toast", TOAST_TSX, &["surface"], &["props.changed"]);
    let first = app
        .mods_open("toast", SurfacePlace::Toast, json!({}), None, None, None)
        .unwrap();
    let second = app
        .mods_open("toast", SurfacePlace::Toast, json!({}), None, None, None)
        .unwrap();
    // 未指定 surface 走默认（第一条提示表面）。
    app.push_mod_notification("default", "", "info", 30, None, None, None, false);
    assert!(
        wait_until(&mut app, 20.0, |app| {
            toasts_len(app, &first) == Some(1) && toasts_len(app, &second) == Some(0)
        }),
        "默认提示表面没有收到 props"
    );
    println!(
        "default: first = {}, second = {}",
        toast_props(&app, &first)["toasts"],
        toast_props(&app, &second)["toasts"]
    );
    assert_eq!(
        toast_props(&app, &first)["toasts"].as_array().map(Vec::len),
        Some(1)
    );
    assert_eq!(
        toast_props(&app, &second)["toasts"]
            .as_array()
            .map(Vec::len),
        Some(0)
    );
    // 指定 surface 只投给那一条。
    app.push_mod_notification("routed", "", "warn", 30, None, None, Some(&second), false);
    assert!(
        wait_until(&mut app, 20.0, |app| {
            toasts_len(app, &first) == Some(1) && toasts_len(app, &second) == Some(1)
        }),
        "指定提示表面没有收到 props"
    );
    println!(
        "routed to {}: first = {}, second = {}",
        second,
        toast_props(&app, &first)["toasts"],
        toast_props(&app, &second)["toasts"]
    );
    assert_eq!(
        toast_props(&app, &first)["toasts"].as_array().map(Vec::len),
        Some(1)
    );
    assert_eq!(
        toast_props(&app, &second)["toasts"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
}

#[test]
fn toast_ticket_package_place_opens_surface() {
    let mut app = app();
    std::fs::create_dir_all(app.config.mods_dir()).unwrap();
    write_mod(&app, "toast", TOAST_TSX, &["surface"], &["props.changed"]);
    // 清单声明 place = "toast"：包启用后自动有提示表面，不占外框停靠区。
    let manifest = app.config.mods_dir().join("toast").join("hctui.toml");
    let text = std::fs::read_to_string(&manifest)
        .unwrap()
        .replace("host_api = 1", "place = \"toast\"\nhost_api = 1");
    std::fs::write(&manifest, text).unwrap();
    assert!(
        wait_until(&mut app, 20.0, |app| app.mods.surfaces.iter().any(
            |surface| surface.mod_id == "toast" && surface.place == SurfacePlace::Toast
        )),
        "清单声明 place = \"toast\" 的包没有自动打开提示表面"
    );
    let surfaces: Vec<(String, SurfacePlace)> = app
        .mods
        .surfaces
        .iter()
        .map(|surface| (surface.mod_id.clone(), surface.place))
        .collect();
    println!("surfaces = {surfaces:?}, docked = {:?}", app.mods_docked);
    assert!(surfaces
        .iter()
        .any(|(id, place)| id == "toast" && *place == SurfacePlace::Toast));
}
