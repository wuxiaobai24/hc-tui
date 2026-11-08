//! crashpane 验收：模块崩溃留错误占位。
//!
//! region（sidebar / tabs / status）首次失败即在同一矩形里画红框错误盒，覆盖层
//! 保尺寸不自动关闭；`r` 重载、`?` 写详情、同一错误只记一条日志一次 toast。
//! 构造方式沿用 `tests/m9b_interaction.rs`：临时配置目录 + `TestBackend` 帧循环。

use std::path::{Path, PathBuf};
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use hc_tui::app::App;
use hc_tui::config::Config;
use hc_tui::model::{Tab, Workspace};
use hc_tui::ui;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

const STATUS_TSX: &str = include_str!("../examples/mods/status/App.tsx");
const STATUS_MANIFEST: &str = include_str!("../examples/mods/status/hctui.toml");
const STATUS_NSH: &str = include_str!("../examples/mods/status/nsh.json");
const SIDEBAR_MANIFEST: &str = include_str!("../examples/mods/sidebar/hctui.toml");
const SIDEBAR_NSH: &str = include_str!("../examples/mods/sidebar/nsh.json");

/// 渲染期崩溃：每次 `render` 都抛，宿主应当在同一矩形里留占位。
const STATUS_CRASH_TSX: &str = r#"
import { mount, Text, View } from "nsh";

function App() {
  throw new Error("crashpane probe: status render exploded");
  return <View><Text>unreachable</Text></View>;
}

mount(App);
"#;

/// 覆盖层用的清单：id 与安装目录同名，`place = "none"` 不参与默认外框绑定。
const DIALOG_MANIFEST: &str = "[mod]\nid = \"crashdlg\"\ntitle = \"Crash Dialog\"\nplace = \"none\"\nhost_api = 1\n\n[grants]\nhost = [\"state\"]\n";

/// 加载期崩溃：bundle 顶层直接抛，表面没有可用引擎。
const DIALOG_CRASH_TSX: &str = r#"
import { mount, Text, View } from "nsh";

throw new Error("crashpane probe: dialog load exploded");

mount(() => <View><Text>unreachable</Text></View>);
"#;

fn unique_dir(tag: &str) -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!(
        "hctui-crashpane-{tag}-{}-{unique}",
        std::process::id()
    ))
}

fn config_in(root: &Path) -> Config {
    Config {
        restore: false,
        prompt_new_tab: false,
        config_path: root.join("config").join("config.toml"),
        state_path: root.join("state").join("state.json"),
        ..Config::default()
    }
}

fn install_module(
    config: &Config,
    id: &str,
    manifest: &str,
    nsh: &str,
    capabilities: &str,
    tsx: &str,
) {
    let staging = unique_dir(&format!("src-{id}"));
    std::fs::create_dir_all(staging.join("dist")).expect("staging dist");
    std::fs::write(staging.join("hctui.toml"), manifest).expect("staging manifest");
    std::fs::write(staging.join("nsh.json"), nsh).expect("staging nsh.json");
    std::fs::write(staging.join("dist").join("capabilities.json"), capabilities)
        .expect("staging capabilities");
    let code = nsh_bundler::compile_module("App.tsx", tsx, &[]).expect("module compiles");
    std::fs::write(staging.join("dist").join("bundle.js"), code).expect("staging bundle");
    hc_tui::mods::store::install_local(config, &staging).expect("module installs");
    std::fs::remove_dir_all(staging).ok();
}

fn fixture_state(app: &mut App) {
    app.note.clear();
    app.connection = hc_tui::app::Connection::Connecting;
    app.workspaces = vec![
        Workspace {
            origin: None,
            machine: None,
            folder: Some("/nonexistent/crashpane-one".to_string()),
            id: 1,
            label: "one".to_string(),
            tabs: vec![Tab::new(11, 101), Tab::new(12, 102)],
            active: 0,
        },
        Workspace {
            origin: None,
            machine: None,
            folder: Some("/nonexistent/crashpane-two".to_string()),
            id: 2,
            label: "two".to_string(),
            tabs: vec![Tab::new(21, 201)],
            active: 0,
        },
    ];
    app.active_ws = 0;
}

fn app_with(modules: &[(&str, &str, &str, &str, &str)]) -> (App, PathBuf, Config) {
    let root = unique_dir("app");
    let config = config_in(&root);
    for (id, manifest, nsh, capabilities, tsx) in modules {
        install_module(&config, id, manifest, nsh, capabilities, tsx);
    }
    let mut app = App::new(config.clone()).expect("app builds");
    fixture_state(&mut app);
    (app, root, config)
}

fn screen(app: &mut App, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal.draw(|frame| ui::draw(frame, app)).expect("draw");
    let buffer = terminal.backend().buffer();
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                + "\n"
        })
        .collect()
}

fn frames(app: &mut App, count: usize) -> String {
    let mut frame = String::new();
    for _ in 0..count {
        frame = screen(app, 80, 24);
        app.pump_mods();
        std::thread::sleep(Duration::from_millis(5));
    }
    frame
}

/// 帧转录按格取符号，宽字符的后半格是空格；断言前把空格去掉再比。
fn squeezed(text: &str) -> String {
    text.replace(' ', "")
}

fn key(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::empty()));
}

fn click(app: &mut App, column: u16, row: u16) {
    app.on_mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::empty(),
    });
    app.on_mouse(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column,
        row,
        modifiers: KeyModifiers::empty(),
    });
}

/// region 失败后的证据：占位帧、日志行、通知条数、`?` 详情与 `r` 恢复。
#[test]
fn crashing_status_region_leaves_an_error_box_and_recovers_on_r() {
    hc_tui::i18n::set_zh(true);
    let (mut app, root, config) = app_with(&[(
        "status",
        STATUS_MANIFEST,
        STATUS_NSH,
        "{\"host\":[\"state\"]}",
        STATUS_CRASH_TSX,
    )]);
    let frame = frames(&mut app, 60);
    println!("--- placeholder frame ---\n{frame}");

    let status_row = squeezed(frame.lines().last().unwrap_or_default());
    assert!(
        status_row.contains("status·渲染失败"),
        "row: {status_row:?}"
    );
    // 一行高的模式行放不下完整首行：按剩余宽度截断，标题与提示保留。
    assert!(status_row.contains("crash"), "row: {status_row:?}");
    assert!(status_row.contains("r重载·?详情"), "row: {status_row:?}");

    let surface = app
        .mods
        .surfaces
        .iter()
        .find(|surface| surface.mod_id == "status")
        .expect("status surface stays");
    assert_eq!(
        surface.state,
        hc_tui::mods::SurfaceState::Failed,
        "failure must not unload the surface"
    );
    assert_eq!(app.focused_mod, None, "placeholder must not take focus");

    // 连续每帧失败只记一条日志、只弹一次 toast。
    let log = hc_tui::store::Store::open(config.mods_state_dir())
        .log_tail("status", 400)
        .expect("engine log");
    println!("--- packages log ---\n{log}");
    println!(
        "--- 每帧失败后的日志行数：{}（含栈）---",
        log.lines().count()
    );
    assert_eq!(
        log.lines()
            .filter(|line| line.contains("crashpane probe: status render exploded"))
            .count(),
        1,
        "one log line per module and error: {log}"
    );
    let toasts = app
        .mod_notifications
        .iter()
        .filter(|item| item.text.contains("crashpane probe"))
        .count();
    assert_eq!(toasts, 1, "one toast per module and error");

    // 坏模块上按 r：占位保留；连续每帧失败不再刷屏（每次尝试各一条）。
    let surface_id = surface.id.clone();
    key(&mut app, KeyCode::Char('r'));
    frames(&mut app, 6);
    let attempts = hc_tui::store::Store::open(config.mods_state_dir())
        .log_tail("status", 400)
        .expect("engine log")
        .lines()
        .filter(|line| line.contains("crashpane probe: status render exploded"))
        .count();
    frames(&mut app, 20);
    let after = hc_tui::store::Store::open(config.mods_state_dir())
        .log_tail("status", 400)
        .expect("engine log")
        .lines()
        .filter(|line| line.contains("crashpane probe: status render exploded"))
        .count();
    assert_eq!(
        attempts, after,
        "consecutive failed frames must not add log lines"
    );
    let surface = app.mods.surface(&surface_id).expect("surface stays");
    assert_eq!(surface.state, hc_tui::mods::SurfaceState::Failed);

    // `r` 重载该模块：换成不崩的版本后恢复正常帧。
    install_module(
        &config,
        "status",
        STATUS_MANIFEST,
        STATUS_NSH,
        "{\"host\":[\"state\"]}",
        STATUS_TSX,
    );
    key(&mut app, KeyCode::Char('r'));
    let recovered = frames(&mut app, 120);
    println!("--- recovered frame ---\n{recovered}");
    let row = squeezed(recovered.lines().last().unwrap_or_default());
    assert!(!row.contains("渲染失败"), "row: {row:?}");
    assert!(row.contains("连接中"), "row: {row:?}");
    std::fs::remove_dir_all(root).ok();
}

/// 状态机的另一条恢复路径：重装（包记录变了）后自动按新代际恢复，无需按 r。
#[test]
fn reinstalling_the_package_recovers_the_region_without_a_key() {
    hc_tui::i18n::set_zh(true);
    let (mut app, root, config) = app_with(&[(
        "status",
        STATUS_MANIFEST,
        STATUS_NSH,
        "{\"host\":[\"state\"]}",
        STATUS_CRASH_TSX,
    )]);
    let frame = frames(&mut app, 60);
    assert!(
        squeezed(&frame).contains("status·渲染失败"),
        "frame: {frame}"
    );

    install_module(
        &config,
        "status",
        STATUS_MANIFEST,
        STATUS_NSH,
        "{\"host\":[\"state\"]}",
        STATUS_TSX,
    );
    app.refresh_plugins_if_changed();
    let recovered = frames(&mut app, 120);
    println!("--- reinstall recovery frame ---\n{recovered}");
    let row = squeezed(recovered.lines().last().unwrap_or_default());
    assert!(!row.contains("渲染失败"), "row: {row:?}");
    assert!(row.contains("连接中"), "row: {row:?}");
    std::fs::remove_dir_all(root).ok();
}

/// `?`：完整异常（模块、宿主方法、调用序号）写进 `packages log`。
#[test]
fn question_mark_writes_the_full_error_context_to_the_package_log() {
    hc_tui::i18n::set_zh(true);
    let (mut app, root, config) = app_with(&[(
        "status",
        STATUS_MANIFEST,
        STATUS_NSH,
        "{\"host\":[\"state\"]}",
        STATUS_CRASH_TSX,
    )]);
    frames(&mut app, 60);
    key(&mut app, KeyCode::Char('?'));
    let log = hc_tui::store::Store::open(config.mods_state_dir())
        .log_tail("status", 400)
        .expect("engine log");
    println!("--- packages log after ? ---\n{log}");
    assert!(log.contains("detail mod status surface s"), "log: {log}");
    assert!(log.contains("detail last host call #"), "log: {log}");
    assert!(
        log.contains("detail error Error: crashpane probe: status render exploded"),
        "log: {log}"
    );
    std::fs::remove_dir_all(root).ok();
}

/// 侧栏占位时命中域照旧：点侧栏第 5 行仍选中第二个 Space。
#[test]
fn crashing_sidebar_keeps_its_click_hit_domains() {
    hc_tui::i18n::set_zh(true);
    let (mut app, root, _config) = app_with(&[(
        "sidebar",
        SIDEBAR_MANIFEST,
        SIDEBAR_NSH,
        "{\"host\":[\"state\"]}",
        STATUS_CRASH_TSX,
    )]);
    let frame = frames(&mut app, 60);
    println!("--- sidebar placeholder frame ---\n{frame}");
    assert!(
        squeezed(&frame).contains("sidebar·渲染失败"),
        "sidebar box missing: {frame}"
    );
    assert_eq!(app.active_ws, 0);
    // 侧栏第 3 行以下是 Space 行：第一行标题、空行、机器组标题、` one`、` two`。
    click(&mut app, 4, 4);
    assert_eq!(app.active_ws, 1, "click must still select the Space");
    assert!(
        app.mods
            .surfaces
            .iter()
            .any(|surface| surface.mod_id == "sidebar"
                && surface.state == hc_tui::mods::SurfaceState::Failed),
        "sidebar surface stays in placeholder"
    );
    std::fs::remove_dir_all(root).ok();
}

/// 覆盖层：加载失败也保尺寸显示错误盒，`Esc` 关掉后不自动重开。
#[test]
fn crashing_dialog_keeps_its_box_and_closes_on_esc() {
    hc_tui::i18n::set_zh(true);
    let (mut app, root, _config) = app_with(&[(
        "crashdlg",
        DIALOG_MANIFEST,
        STATUS_NSH,
        "{\"host\":[\"state\"]}",
        DIALOG_CRASH_TSX,
    )]);
    frames(&mut app, 3);
    let id = app
        .mods_open(
            "crashdlg",
            hc_tui::mods::SurfacePlace::Dialog,
            serde_json::json!({}),
            Some((60, 10)),
            None,
            None,
        )
        .expect("a failed module still opens a placeholder surface");
    let before = frames(&mut app, 60);
    println!("--- dialog placeholder frame ---\n{before}");
    let box_frame = squeezed(&before);
    assert!(box_frame.contains("crashdlg·渲染失败"), "frame: {before}");
    assert!(box_frame.contains("r重载·?详情"), "frame: {before}");
    let rect = app.mods.surface(&id).expect("surface stays").area;
    assert!(rect.width > 20 && rect.height == 10, "rect: {rect:?}");
    assert_eq!(
        app.mods.surface(&id).expect("surface stays").area,
        rect,
        "the placeholder keeps the surface size"
    );

    key(&mut app, KeyCode::Esc);
    assert!(
        app.mods.surface(&id).is_none(),
        "Esc closes the placeholder"
    );
    let after = frames(&mut app, 30);
    println!("--- frame after Esc ---\n{after}");
    assert!(
        !squeezed(&after).contains("crashdlg·渲染失败"),
        "frame: {after}"
    );
    assert!(
        app.mods.surface(&id).is_none(),
        "a closed placeholder must not reopen on its own"
    );
    std::fs::remove_dir_all(root).ok();
}
