//! M9g toast 对拍：模块提示层与内置 toast 在同一内容区逐格比较字符格。
//!
//! 四个状态（一条 info / 一条 error / 三条堆叠 / 长文本截断）× 80×24 与 120×40。
//! 另外四条：点击某条跑动作并撤销、点空白不吞事件、超时后条目消失、
//! `--no-mods` 回落内置、模块接手后不出现两份。
//!
//! 颜色不是对拍对象：nsh TUI 把前景归到终端 16 色/Reset，内置实现写主题
//! RGB，逐格比色永远不相等（仓库既有 `scripts/mods-parity.sh` 同样只比字符格）。

mod support;

use std::path::PathBuf;

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use hc_tui::app::App;
use hc_tui::config::Config;
use hc_tui::mods::SurfacePlace;
use hc_tui::ui;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::Terminal;
use serde_json::{json, Value};

const TOAST_TSX: &str = include_str!("../examples/mods/toast/App.tsx");
const TOAST_MANIFEST: &str = include_str!("../examples/mods/toast/hctui.toml");
const TOAST_NSH: &str = include_str!("../examples/mods/toast/nsh.json");
const TOAST_CAPABILITIES: &str = r#"{"host":["run","dismiss"]}"#;

/// 反例：宽度算少一格，对拍必须报出差异。
const WIDTH_OLD: &str = "Math.min([...text].length + 4, cells)";
const WIDTH_NEW: &str = "Math.min([...text].length + 3, cells)";

/// 驱动模块侧通知的调用方：按 `props.script` 顺序调用宿主方法，做完关掉自己。
const CALLER_TSX: &str = r#"
import { host, mount, signal, Text, View } from "nsh";

const lines = signal<string[]>([]);
const out: string[] = [];

async function run(steps: any[]) {
  for (const step of steps) {
    try {
      const reply = await host.call(step.name, step.data ?? {});
      out.push((step.label ?? step.name) + " => " + JSON.stringify(reply));
    } catch (err) {
      out.push((step.label ?? step.name) + " !! " + (err as Error).message);
    }
  }
  lines.set(out.slice());
  host.call("closeOverlay", { surface: (host.params as any)?.surface }).catch(() => {});
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

/// 取色探针：把三个主题 token 各画一格，读出 nsh 量化后的颜色。
const TOKEN_PROBE_TSX: &str = r##"
import { host, mount, Text, View } from "nsh";

const params = (host.params ?? {}) as any;
const tokens: Record<string, string> = params.theme?.tokens ?? {};

function App() {
  return (
    <View style={{ flex: 1, flexDirection: "column" }}>
      <Text style={{ color: tokens.accent ?? "#89b4fa" }}>{"┌─┐"}</Text>
      <Text style={{ color: tokens.agent_working ?? "#f9e2af" }}>{"┌─┐"}</Text>
      <Text style={{ color: tokens.agent_error ?? "#f38ba8" }}>{"┌─┐"}</Text>
    </View>
  );
}

mount(App);
"##;

const CALLER_MANIFEST: &str = "[mod]\nid = \"caller\"\nplace = \"none\"\nhost_api = 1\n\n[grants]\nhost = [\"notify\", \"run\", \"dismiss\", \"closeOverlay\"]\n";
const CALLER_NSH: &str = r#"{"name":"Caller","id":"dev.hctui.mods.caller","version":"1.0.0","build":1,"capabilities":{"host":["notify","run","dismiss","closeOverlay"]}}"#;
const CALLER_CAPABILITIES: &str = r#"{"host":["notify","run","dismiss","closeOverlay"]}"#;

fn unique_dir(tag: &str) -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!("hctui-m9g-{tag}-{}-{unique}", std::process::id()))
}

fn config_in(root: &std::path::Path) -> Config {
    Config {
        restore: false,
        prompt_new_tab: false,
        config_path: root.join("config").join("config.toml"),
        state_path: root.join("state").join("state.json"),
        theme: hc_tui::theme::builtin("catppuccin").expect("theme"),
        ..Config::default()
    }
}

fn write_module(config: &Config, id: &str, tsx: &str, manifest: &str, nsh: &str, caps: &str) {
    let staging = unique_dir(&format!("stage-{id}"));
    std::fs::create_dir_all(staging.join("dist")).expect("staging dist");
    std::fs::write(staging.join("hctui.toml"), manifest).expect("manifest");
    std::fs::write(staging.join("nsh.json"), nsh).expect("nsh.json");
    std::fs::write(staging.join("dist").join("capabilities.json"), caps).expect("capabilities");
    let code = nsh_bundler::compile_module("App.tsx", tsx, &[]).expect("module compiles");
    std::fs::write(staging.join("dist").join("bundle.js"), code).expect("bundle");
    hc_tui::mods::store::install_local(config, &staging).expect("module installs");
    std::fs::remove_dir_all(staging).ok();
}

/// 对拍用的提示模块；`M9G_NEGATIVE` 时把框宽算少一格。
fn toast_source() -> String {
    if std::env::var_os("M9G_NEGATIVE").is_some() {
        let broken = TOAST_TSX.replace(WIDTH_OLD, WIDTH_NEW);
        assert_ne!(broken, TOAST_TSX, "negative control marker is missing");
        broken
    } else {
        TOAST_TSX.to_string()
    }
}

fn install_toast(config: &Config) {
    write_module(
        config,
        "toast",
        &toast_source(),
        TOAST_MANIFEST,
        TOAST_NSH,
        TOAST_CAPABILITIES,
    );
    write_module(
        config,
        "caller",
        CALLER_TSX,
        CALLER_MANIFEST,
        CALLER_NSH,
        CALLER_CAPABILITIES,
    );
}

fn install_probe(config: &Config) {
    write_module(
        config,
        "probe",
        TOKEN_PROBE_TSX,
        "[mod]\nid = \"probe\"\nplace = \"none\"\nhost_api = 1\n\n[grants]\nhost = []\n",
        r#"{"name":"Probe","id":"dev.hctui.mods.probe","version":"1.0.0","build":1,"capabilities":{"host":[]}}"#,
        r#"{"host":[]}"#,
    );
}

fn install_caller(config: &Config) {
    write_module(
        config,
        "caller",
        CALLER_TSX,
        CALLER_MANIFEST,
        CALLER_NSH,
        CALLER_CAPABILITIES,
    );
}

fn region_text(buffer: &Buffer, area: Rect) -> String {
    (area.y..area.bottom())
        .map(|y| {
            (area.x..area.right())
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                + "\n"
        })
        .collect()
}

struct Harness {
    app: App,
    terminal: Terminal<TestBackend>,
    content: Rect,
}

impl Harness {
    fn new(config: Config, width: u16, height: u16) -> Self {
        let mut app = App::new(config).expect("app builds");
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        terminal
            .draw(|frame| ui::draw(frame, &mut app))
            .expect("draw");
        let content = app.content;
        Self {
            app,
            terminal,
            content,
        }
    }

    fn pump(&mut self, rounds: usize) {
        for _ in 0..rounds {
            self.terminal
                .draw(|frame| ui::draw(frame, &mut self.app))
                .expect("draw");
            self.app.pump_mods();
            self.app.tick();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// 出帧到条件成立；超时返回 false。
    fn pump_until(&mut self, label: &str, mut ready: impl FnMut(&App) -> bool) -> bool {
        // 慢 runner（尤其 macOS CI）上模块加载可能远超固定轮数：按墙钟等 30 秒。
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while std::time::Instant::now() < deadline {
            self.terminal
                .draw(|frame| ui::draw(frame, &mut self.app))
                .expect("draw");
            self.app.pump_mods();
            self.app.tick();
            std::thread::sleep(std::time::Duration::from_millis(5));
            if ready(&self.app) {
                return true;
            }
        }
        panic!("{label}: 超时未达成");
    }

    /// 出帧到内容区满足 `want`（出现或消失某条通知）；超时 panic。
    /// 判的是屏幕上的字，不是固定帧数：慢 runner 上帧率低也只会多转几轮。
    fn pump_until_region(&mut self, label: &str, mut want: impl FnMut(&str) -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while std::time::Instant::now() < deadline {
            self.terminal
                .draw(|frame| ui::draw(frame, &mut self.app))
                .expect("draw");
            self.app.pump_mods();
            self.app.tick();
            if want(&region_text(&self.frame(), self.content)) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("{label}: 超时未达成");
    }

    /// 真实 `notify` 通道（模块 `notify` 与 `hc-tui notify` 同一入口）。
    fn notify(&mut self, steps: &[Value]) {
        self.app
            .mods_open(
                "caller",
                SurfacePlace::Popup,
                json!({ "script": steps }),
                Some((30, 6)),
                Some((1, 1)),
                None,
            )
            .expect("caller opens");
        let mut caller_gone = false;
        self.pump_until("caller closes", |app| {
            caller_gone |= app
                .mods
                .surfaces
                .iter()
                .all(|surface| surface.mod_id != "caller");
            caller_gone
        });
        // 模块侧的渲染与 props 投递是异步的：再走几帧。
        self.pump(30);
    }

    fn frame(&self) -> Buffer {
        self.terminal.backend().buffer().clone()
    }

    fn toast_props(&self) -> Value {
        self.app
            .mods
            .surfaces
            .iter()
            .find(|surface| surface.place == SurfacePlace::Toast)
            .map(|surface| surface.props.clone())
            .unwrap_or(Value::Null)
    }
}

fn has_toast_surface(app: &App) -> bool {
    app.mods
        .surfaces
        .iter()
        .any(|surface| surface.place == SurfacePlace::Toast)
}

fn notify_step(level: &str, text: &str, timeout: u64) -> Value {
    json!({"name": "notify", "data": {
        "title": text, "body": "", "level": level, "timeout": timeout
    }})
}

/// 内置宽度公式：码点数 + 4，夹到内容区，最短 12 列。
fn builtin_width(text: &str, content: Rect) -> u16 {
    ((text.chars().count() as u16) + 4)
        .min(content.width)
        .max(12)
}

fn builtin_box(content: Rect, text: &str) -> Rect {
    let width = builtin_width(text, content);
    Rect::new(
        content.x + content.width - width,
        content.y + content.height - 3,
        width,
        3,
    )
}

/// 一个状态的期望内容：级别与文本。
struct State {
    name: &'static str,
    entries: Vec<(&'static str, String)>,
}

fn states() -> Vec<State> {
    vec![
        State {
            name: "info",
            entries: vec![("info", "connected to build-server".to_string())],
        },
        State {
            name: "error",
            entries: vec![("error", "core rpc failed: connection reset".to_string())],
        },
        State {
            name: "stack",
            entries: vec![
                ("info", "deploy finished".to_string()),
                ("warn", "agent needs input".to_string()),
                ("error", "upload rejected: 413".to_string()),
            ],
        },
        State {
            name: "long",
            entries: vec![(
                "info",
                "a very long notification text that no toast box can hold on one line \
                 because it keeps going and going"
                    .to_string(),
            )],
        },
    ]
}

/// 模块侧的帧：装好提示模块，按状态推通知。
fn module_frame(config: Config, width: u16, height: u16, state: &State) -> (Buffer, Rect) {
    let mut harness = Harness::new(config, width, height);
    harness.pump_until("toast surface opens", has_toast_surface);
    let steps: Vec<Value> = state
        .entries
        .iter()
        .map(|(level, text)| notify_step(level, text, 60))
        .collect();
    harness.notify(&steps);
    let expected = state.entries.len();
    harness.pump_until("toast props arrive", |app| {
        app.mod_notifications.len() == expected && has_toast_surface(app)
    });
    harness.pump(10);
    for surface in &harness.app.mods.surfaces {
        let errors = surface
            .errors
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        println!(
            "surface {} place={:?} area={:?} entries={} errors={:?} last={:?}",
            surface.id,
            surface.place,
            surface.area,
            surface.props["toasts"]
                .as_array()
                .map(Vec::len)
                .unwrap_or(0),
            errors,
            surface.last_error
        );
    }
    let content = harness.content;
    (harness.frame(), content)
}

/// 内置侧的帧：不装提示模块，同一条 `notify` 落到内置 toast。
fn builtin_frame(
    config: Config,
    width: u16,
    height: u16,
    entries: &[(&'static str, String)],
) -> (Buffer, Rect) {
    let mut harness = Harness::new(config, width, height);
    let steps: Vec<Value> = entries
        .iter()
        .map(|(level, text)| notify_step(level, text, 60))
        .collect();
    harness.notify(&steps);
    harness.pump_until("builtin toast", |app| {
        app.mod_notifications.len() == entries.len()
    });
    harness.pump(10);
    let content = harness.content;
    (harness.frame(), content)
}

#[test]
fn toast_matches_builtin_cell_for_cell() {
    let _core = support::IsolatedCore::new("m9g");
    hc_tui::i18n::set_zh(false);
    let mut failures = 0;
    for (width, height) in [(80u16, 24u16), (120, 40)] {
        for state in states() {
            let root = unique_dir(&format!("parity-{}-{width}x{height}", state.name));
            let module_config = config_in(&root);
            install_toast(&module_config);
            let (module, content) = module_frame(module_config, width, height, &state);

            let builtin_root = unique_dir(&format!("builtin-{}-{width}x{height}", state.name));
            let builtin_config = config_in(&builtin_root);
            install_caller(&builtin_config);
            let label = format!("toast {width}x{height} {}", state.name);
            if state.entries.len() == 1 {
                let (level, text) = &state.entries[0];
                let (builtin, builtin_content) =
                    builtin_frame(builtin_config, width, height, &[(level, text.clone())]);
                assert_eq!(content, builtin_content, "content rect must match");
                failures += compare_rects(&label, &builtin, content, &module, content);
            } else {
                // 内置只画一条：每一条单独成帧，按堆叠位置搬过去逐格比。
                let mut boxes: Vec<Rect> = Vec::new();
                for (index, (level, text)) in state.entries.iter().enumerate() {
                    let (builtin, _) = builtin_frame(
                        config_in(&builtin_root),
                        width,
                        height,
                        &[(level, text.clone())],
                    );
                    let source = builtin_box(content, text);
                    let target = Rect::new(
                        source.x,
                        content.y + content.height - 3 * (index as u16 + 1),
                        source.width,
                        3,
                    );
                    boxes.push(target);
                    failures += compare_rects(
                        &format!("{label} entry{index}"),
                        &builtin,
                        source,
                        &module,
                        target,
                    );
                }
                // 盒子之外的内容区与空队列的内置帧一致。
                let (empty, _) = builtin_frame(config_in(&builtin_root), width, height, &[]);
                failures +=
                    compare_masked(&format!("{label} rest"), &empty, &module, content, &boxes);
            }
            std::fs::remove_dir_all(root).ok();
            std::fs::remove_dir_all(builtin_root).ok();
        }
    }
    assert_eq!(failures, 0, "toast module differs from the builtin toast");
}

/// 两块同尺寸区域逐格比字符。
fn compare_rects(
    label: &str,
    builtin: &Buffer,
    builtin_area: Rect,
    module: &Buffer,
    module_area: Rect,
) -> usize {
    let mut differences = 0;
    let mut first: Option<String> = None;
    for y in 0..builtin_area.height {
        for x in 0..builtin_area.width {
            let left = builtin[(builtin_area.x + x, builtin_area.y + y)].symbol();
            let right = module[(module_area.x + x, module_area.y + y)].symbol();
            if left == right {
                continue;
            }
            differences += 1;
            if first.is_none() {
                first = Some(format!(
                    "({},{}) builtin={left:?} module={right:?}",
                    module_area.x + x,
                    module_area.y + y
                ));
            }
        }
    }
    let total = usize::from(builtin_area.width) * usize::from(builtin_area.height);
    println!("{label}: {differences} of {total} cells differ");
    if let Some(line) = first {
        println!("[{label}] first difference {line}");
        println!("[{label}] builtin:\n{}", region_text(builtin, builtin_area));
        println!("[{label}] module:\n{}", region_text(module, module_area));
    }
    differences
}

/// 跳过给定盒子的逐格比较。
fn compare_masked(
    label: &str,
    builtin: &Buffer,
    module: &Buffer,
    area: Rect,
    skip: &[Rect],
) -> usize {
    let mut differences = 0;
    let mut first: Option<String> = None;
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            if skip
                .iter()
                .any(|rect| x >= rect.x && x < rect.right() && y >= rect.y && y < rect.bottom())
            {
                continue;
            }
            let left = builtin[(x, y)].symbol();
            let right = module[(x, y)].symbol();
            if left == right {
                continue;
            }
            differences += 1;
            if first.is_none() {
                first = Some(format!("({x},{y}) builtin={left:?} module={right:?}"));
            }
        }
    }
    println!("{label}: {differences} cells differ outside the boxes");
    if let Some(line) = first {
        println!("[{label}] first difference {line}");
    }
    differences
}

/// 反例：框宽算少一格，对拍必须报出差异。
#[test]
fn wrong_width_is_caught() {
    let _core = support::IsolatedCore::new("m9g");
    hc_tui::i18n::set_zh(false);
    let root = unique_dir("negative");
    let config = config_in(&root);
    write_module(
        &config,
        "toast",
        &TOAST_TSX.replace(WIDTH_OLD, WIDTH_NEW),
        TOAST_MANIFEST,
        TOAST_NSH,
        TOAST_CAPABILITIES,
    );
    install_caller(&config);
    let state = State {
        name: "info",
        entries: vec![("info", "connected to build-server".to_string())],
    };
    let (module, content) = module_frame(config, 80, 24, &state);
    let builtin_root = unique_dir("negative-builtin");
    let builtin_config = config_in(&builtin_root);
    install_caller(&builtin_config);
    let (builtin, _) = builtin_frame(
        builtin_config,
        80,
        24,
        &[("info", "connected to build-server".to_string())],
    );
    let differences = compare_rects(
        "toast negative control",
        &builtin,
        content,
        &module,
        content,
    );
    assert!(
        differences > 0,
        "a box one cell narrower must show up as differing cells"
    );
    std::fs::remove_dir_all(root).ok();
    std::fs::remove_dir_all(builtin_root).ok();
}

/// 点击某条：先 `run`（`target` 作 `terminal`），再 `dismiss`。
#[test]
fn click_runs_action_then_dismisses() {
    let _core = support::IsolatedCore::new("m9g");
    hc_tui::i18n::set_zh(false);
    let root = unique_dir("click");
    let config = config_in(&root);
    install_toast(&config);
    let mut harness = Harness::new(config, 80, 24);
    harness.pump_until("toast surface opens", has_toast_surface);
    harness.notify(&[json!({"name": "notify", "data": {
        "title": "agent blocked", "body": "", "level": "warn", "timeout": 60,
        "action": {"name": "agent.focus"}, "target": "local/t7"
    }})]);
    harness.pump_until("props arrive", |app| app.mod_notifications.len() == 1);
    harness.pump(10);
    let content = harness.content;
    let before = region_text(&harness.frame(), content);
    assert!(before.contains("agent blocked"), "frame:\n{before}");

    // 点框中间那一行：模块的 onPress 命中。
    let area = builtin_box(content, "agent blocked");
    let x = area.x + area.width / 2;
    let y = area.y + 1;
    for kind in [
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Up(MouseButton::Left),
    ] {
        harness.app.on_mouse(MouseEvent {
            kind,
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        });
        harness.pump(20);
    }
    harness
        .pump_until("notification dismissed", |app| {
            app.mod_notifications.is_empty()
        })
        .then_some(())
        .unwrap();
    harness.pump(10);

    let request = harness
        .app
        .mods
        .requests
        .values()
        .find(|request| request.action == "agent.focus")
        .expect("toast module issued the action");
    println!(
        "run action={} args={} status={}",
        request.action, request.args, request.status
    );
    assert_eq!(
        request.args.get("terminal").and_then(Value::as_str),
        Some("local/t7"),
        "target became the terminal argument"
    );
    let after = region_text(&harness.frame(), content);
    assert!(
        !after.contains("agent blocked"),
        "entry left the frame:\n{after}"
    );
    assert_eq!(
        harness.toast_props()["toasts"].as_array().map(Vec::len),
        Some(0)
    );
    std::fs::remove_dir_all(root).ok();
}

/// 点内容区别处：模块认不下这一下，通知留着，事件继续往下传。
#[test]
fn click_outside_the_box_keeps_the_toast() {
    let _core = support::IsolatedCore::new("m9g");
    hc_tui::i18n::set_zh(false);
    let root = unique_dir("outside");
    let config = config_in(&root);
    install_toast(&config);
    let mut harness = Harness::new(config, 80, 24);
    harness.pump_until("toast surface opens", has_toast_surface);
    harness.notify(&[notify_step("info", "keep me", 60)]);
    harness.pump_until("props arrive", |app| app.mod_notifications.len() == 1);
    harness.pump(10);
    let content = harness.content;
    let area = builtin_box(content, "keep me");
    let x = content.x + 2;
    let y = content.y + 2;
    assert!(!(x >= area.x && x < area.right() && y >= area.y && y < area.bottom()));
    harness.app.on_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: x,
        row: y,
        modifiers: KeyModifiers::NONE,
    });
    harness.pump(20);
    println!(
        "after click at ({x},{y}): notifications={} frame has text={}",
        harness.app.mod_notifications.len(),
        region_text(&harness.frame(), content).contains("keep me")
    );
    assert_eq!(harness.app.mod_notifications.len(), 1);
    assert!(region_text(&harness.frame(), content).contains("keep me"));
    std::fs::remove_dir_all(root).ok();
}

/// 超时：`timeout=1` 秒后条目由宿主撤掉，模块跟着消失。
#[test]
fn timeout_removes_the_entry() {
    let _core = support::IsolatedCore::new("m9g");
    hc_tui::i18n::set_zh(false);
    let root = unique_dir("timeout");
    let config = config_in(&root);
    install_toast(&config);
    let mut harness = Harness::new(config, 120, 40);
    harness.pump_until("toast surface opens", has_toast_surface);
    harness.notify(&[notify_step("warn", "expiring entry", 3)]);
    harness.pump_until("props arrive", |app| app.mod_notifications.len() == 1);
    harness.pump_until_region("entry reaches the toast surface", |text| {
        text.contains("expiring entry")
    });
    let content = harness.content;
    let before = region_text(&harness.frame(), content);
    println!("before timeout:\n{}", tail_lines(&before, 4));
    assert!(before.contains("expiring entry"));
    // 到期不靠真实等 3 秒：把宿主时钟拨过超时，`tick` 按拨后的时刻判定。
    harness.app.advance_clock(std::time::Duration::from_secs(4));
    harness.pump_until("entry expires", |app| app.mod_notifications.is_empty());
    // 队列清空后模块还要按新 props 重绘擦掉那一条：等画面本身。
    harness.pump_until_region("frame drops the entry", |text| {
        !text.contains("expiring entry")
    });
    let after = region_text(&harness.frame(), content);
    println!(
        "after the clock passes the timeout:\n{}",
        tail_lines(&after, 4)
    );
    println!(
        "queue={} props={}",
        harness.app.mod_notifications.len(),
        harness.toast_props()["toasts"]
    );
    assert!(!after.contains("expiring entry"), "frame:\n{after}");
    assert!(harness.app.mod_notifications.is_empty());
    std::fs::remove_dir_all(root).ok();
}

fn tail_lines(text: &str, count: usize) -> String {
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    lines[lines.len().saturating_sub(count)..].join("\n")
}

/// `--no-mods`：提示模块在盘上也不加载，内置 toast 照旧画。
#[test]
fn no_mods_falls_back_to_the_builtin_toast() {
    let _core = support::IsolatedCore::new("m9g");
    hc_tui::i18n::set_zh(false);
    let root = unique_dir("fallback");
    let mut config = config_in(&root);
    install_toast(&config);
    config.no_mods = true;
    let mut harness = Harness::new(config, 80, 24);
    harness.pump(30);
    assert!(
        harness.app.mods.surfaces.is_empty(),
        "no module surface under --no-mods"
    );
    // 内置槽位就是 `register_notification` 在无提示表面时写的那一格。
    harness.app.toast = Some(hc_tui::app::Toast {
        text: "plain fallback".to_string(),
        level: "error".to_string(),
        until: std::time::Instant::now() + std::time::Duration::from_secs(60),
    });
    harness.pump(5);
    let content = harness.content;
    let text = region_text(&harness.frame(), content);
    println!("--no-mods frame:\n{}", tail_lines(&text, 4));
    assert!(text.contains("plain fallback"));
    assert_eq!(text.matches('┌').count(), 1, "builtin box drawn once");
    std::fs::remove_dir_all(root).ok();
}

/// 模块接手后内置槽位不再另画一份：每条只出现一次，最旧的那条留在最下面。
#[test]
fn module_takes_over_without_a_second_box() {
    let _core = support::IsolatedCore::new("m9g");
    hc_tui::i18n::set_zh(false);
    let root = unique_dir("takeover");
    let config = config_in(&root);
    install_caller(&config);
    let mut harness = Harness::new(config.clone(), 80, 24);
    harness.notify(&[
        notify_step("info", "deploy finished", 60),
        notify_step("error", "upload rejected", 60),
    ]);
    harness.pump(20);
    let content = harness.content;
    let before = region_text(&harness.frame(), content);
    println!(
        "before install: boxes={} builtin={:?}",
        before.matches('┌').count(),
        harness.app.toast.as_ref().map(|toast| toast.text.clone())
    );
    assert_eq!(
        before.matches('┌').count(),
        1,
        "builtin shows the newest only"
    );
    assert!(before.contains("upload rejected"));

    // 装上提示模块并重载注册表：提示表面自动打开，内置槽位让位。
    write_module(
        &config,
        "toast",
        TOAST_TSX,
        TOAST_MANIFEST,
        TOAST_NSH,
        TOAST_CAPABILITIES,
    );
    harness.app.refresh_plugins_if_changed();
    harness.pump_until("toast surface opens", has_toast_surface);
    harness.pump(30);
    let after = region_text(&harness.frame(), content);
    println!(
        "after install: boxes={} builtin={:?} surfaces={}",
        after.matches('┌').count(),
        harness.app.toast.as_ref().map(|toast| toast.text.clone()),
        harness
            .app
            .mods
            .surfaces
            .iter()
            .filter(|surface| surface.place == SurfacePlace::Toast)
            .count()
    );
    println!("{}", tail_lines(&after, 6));
    assert_eq!(after.matches('┌').count(), 2, "two stacked boxes");
    assert_eq!(
        after.matches("deploy finished").count(),
        1,
        "frame:\n{after}"
    );
    assert_eq!(
        after.matches("upload rejected").count(),
        1,
        "frame:\n{after}"
    );
    assert!(harness.app.toast.is_none(), "builtin slot handed over");
    std::fs::remove_dir_all(root).ok();
}

/// 边框色：三个级别各取一条的颜色，必须与同主题 token 的取色探针一致。
#[test]
fn level_selects_the_border_colour() {
    let _core = support::IsolatedCore::new("m9g");
    hc_tui::i18n::set_zh(false);
    let colour_source = if std::env::var_os("M9G_NEGATIVE_COLOR").is_some() {
        let broken = TOAST_TSX.replace(
            "if (level == \"warn\") return token(\"agent_working\", \"#f9e2af\");",
            "if (level == \"warn\") return token(\"accent\", \"#89b4fa\");",
        );
        assert_ne!(
            broken, TOAST_TSX,
            "colour negative control marker is missing"
        );
        broken
    } else {
        TOAST_TSX.to_string()
    };
    let root = unique_dir("colour");
    let config = config_in(&root);
    write_module(
        &config,
        "toast",
        &colour_source,
        TOAST_MANIFEST,
        TOAST_NSH,
        TOAST_CAPABILITIES,
    );
    install_caller(&config);
    install_probe(&config);

    // 探针：accent / agent_working / agent_error 三格的量化颜色。
    let mut harness = Harness::new(config.clone(), 80, 24);
    harness.pump(30);
    harness
        .app
        .mods_open(
            "probe",
            SurfacePlace::Popup,
            json!({}),
            Some((10, 3)),
            Some((2, 2)),
            None,
        )
        .expect("probe opens");
    harness.pump(40);
    let area = harness
        .app
        .mods
        .surfaces
        .iter()
        .find(|surface| surface.mod_id == "probe")
        .map(|surface| surface.area)
        .expect("probe area");
    let frame = harness.frame();
    let probe: Vec<ratatui::style::Color> =
        (0..3).map(|row| frame[(area.x, area.y + row)].fg).collect();
    println!("probe area = {area:?} text=\n{}", region_text(&frame, area));
    println!("probe colour fg = {probe:?}");
    std::fs::remove_dir_all(root).ok();

    // 模块侧：每个级别一条，读框左上角边框格的颜色。
    let mut seen = Vec::new();
    for (index, (level, text)) in [
        ("info", "info border"),
        ("warn", "warn border"),
        ("error", "error border"),
    ]
    .iter()
    .enumerate()
    {
        let root = unique_dir(&format!("colour-{level}"));
        let config = config_in(&root);
        write_module(
            &config,
            "toast",
            &colour_source,
            TOAST_MANIFEST,
            TOAST_NSH,
            TOAST_CAPABILITIES,
        );
        install_caller(&config);
        let mut harness = Harness::new(config, 80, 24);
        harness.pump_until("toast surface opens", has_toast_surface);
        harness.notify(&[notify_step(level, text, 60)]);
        harness.pump_until("props arrive", |app| app.mod_notifications.len() == 1);
        harness.pump(10);
        let box_area = builtin_box(harness.content, text);
        let border = harness.frame()[(box_area.x, box_area.y)].fg;
        println!(
            "{level} border fg = {border:?}, probe token fg = {:?}",
            probe[index]
        );
        assert_eq!(border, probe[index], "{level} border uses the builtin tint");
        seen.push(border);
        std::fs::remove_dir_all(root).ok();
    }
    assert_eq!(seen.len(), 3, "three levels inspected");
}
