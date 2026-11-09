//! M9c settings 对拍：模块浮层与内置浮层在同一个矩形里逐格比较字符格。
//!
//! 两个状态（默认值 / 每段都切到非默认值）× 四个段 × 80×24 与 120×40。
//! 另外三条：键盘与鼠标走模块组件、`--no-mods` 回落内置浮层、主题动作的
//! `theme.changed` 真的投递到模块。
//!
//! 颜色不是对拍对象：nsh TUI 把前景归到终端 16 色/Reset，内置实现写主题
//! RGB，逐格比色永远不相等（仓库既有 `scripts/mods-parity.sh` 同样只比字符格）。

use std::path::{Path, PathBuf};

mod support;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use hc_tui::app::{App, SettingsOverlay};
use hc_tui::config::{Config, Delivery, Indicators};
use hc_tui::ui;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::Terminal;

const SETTINGS_TSX: &str = include_str!("../examples/mods/settings/App.tsx");
const SETTINGS_MANIFEST: &str = include_str!("../examples/mods/settings/hctui.toml");
const SETTINGS_NSH: &str = include_str!("../examples/mods/settings/nsh.json");
const SETTINGS_CAPABILITIES: &str = r#"{"host":["state","run","closeOverlay","surface"]}"#;

/// 故意少渲染一项：用来证明对拍断言真的验到了内容。
const DROPPED_ITEM_OLD: &str = "for (let index = offset; index < last; index++)";
const DROPPED_ITEM_NEW: &str = "for (let index = offset + 1; index < last; index++)";

/// 记录 `theme.changed` 的探针模块：主题事件到达时写进通知队列。
const THEME_PROBE_TSX: &str = r#"
import { host, mount, signal, Text, View } from "nsh";
const seen = signal("waiting");
host.on("theme.changed", (theme: any) => {
  seen.set(`theme.changed:${theme?.name}`);
});
function App() {
  host.call("state", { domains: [] }).catch(() => {});
  return (
    <View style={{ flex: 1 }}>
      <Text>{seen()}</Text>
    </View>
  );
}
mount(App);
"#;

fn unique_dir(tag: &str) -> PathBuf {
    // 序号来自原子计数：并发用例不会在同一纳秒拿到同一个目录。
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let unique = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!("hctui-m9c-{tag}-{}-{unique}", std::process::id()))
}

/// 对拍状态：默认值与「每段都切到非默认值」。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Values {
    Default,
    Changed,
}

fn config_in(root: &Path, values: Values) -> Config {
    let mut config = Config {
        restore: false,
        prompt_new_tab: false,
        config_path: root.join("config").join("config.toml"),
        state_path: root.join("state").join("state.json"),
        theme: hc_tui::theme::builtin("catppuccin").expect("theme"),
        ..Config::default()
    };
    if values == Values::Changed {
        config.theme = hc_tui::theme::builtin("nord").expect("theme");
        config.indicators = Indicators::Symbols;
        config.notifications = Delivery::System;
        config.sound = true;
    }
    config
}

fn write_module(config: &Config, id: &str, tsx: &str, manifest: &str, nsh: &str) {
    let staging = unique_dir(&format!("stage-{id}"));
    std::fs::create_dir_all(staging.join("dist")).expect("staging dist");
    std::fs::write(staging.join("hctui.toml"), manifest).expect("manifest");
    std::fs::write(staging.join("nsh.json"), nsh).expect("nsh.json");
    std::fs::write(
        staging.join("dist").join("capabilities.json"),
        SETTINGS_CAPABILITIES,
    )
    .expect("capabilities");
    let code = nsh_bundler::compile_module("App.tsx", tsx, &[]).expect("module compiles");
    std::fs::write(staging.join("dist").join("bundle.js"), code).expect("bundle");
    hc_tui::mods::store::install_local(config, &staging).expect("module installs");
    std::fs::remove_dir_all(staging).ok();
}

fn install_settings(config: &Config) {
    let source = match std::env::var_os("M9C_NEGATIVE") {
        Some(_) => {
            let broken = SETTINGS_TSX.replace(DROPPED_ITEM_OLD, DROPPED_ITEM_NEW);
            assert_ne!(broken, SETTINGS_TSX, "negative control marker is missing");
            broken
        }
        None => SETTINGS_TSX.to_string(),
    };
    write_module(config, "settings", &source, SETTINGS_MANIFEST, SETTINGS_NSH);
}

fn settings_selected(config: &Config, section: usize) -> usize {
    match section {
        0 => hc_tui::theme::names()
            .iter()
            .position(|name| *name == config.theme.name)
            .unwrap_or(0),
        1 => usize::from(config.indicators == Indicators::Symbols),
        2 => match config.notifications {
            Delivery::Off => 0,
            Delivery::Toast => 1,
            Delivery::System => 2,
        },
        _ => usize::from(config.sound),
    }
}

fn region_text(buffer: &Buffer, area: Rect) -> String {
    support::region_text(buffer, area)
}

/// 内置浮层的帧：打开设置、停在指定段。
fn builtin_frame(config: Config, width: u16, height: u16, section: usize) -> (Buffer, Rect) {
    let mut app = App::new(config.clone()).expect("app builds");
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal
        .draw(|frame| ui::draw(frame, &mut app))
        .expect("draw");
    let content = app.content;
    app.settings = Some(SettingsOverlay {
        section,
        selected: settings_selected(&config, section),
    });
    terminal
        .draw(|frame| ui::draw(frame, &mut app))
        .expect("draw");
    (terminal.backend().buffer().clone(), content)
}

/// 按键：投递后画到模块画出新内容；关闭类按键把整块矩形换掉了也算落地。
fn key(app: &mut App, terminal: &mut Terminal<TestBackend>, content: Rect, code: KeyCode) {
    let before = region_text(terminal.backend().buffer(), content);
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
    support::pump_until_repainted(app, terminal, content, &before);
}

/// `prefix+s`：`keys.settings` 命名绑定，与 `app.settings` 动作同一入口。
fn prefix_settings(app: &mut App) {
    app.on_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
    app.on_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
}

/// 模块浮层的帧：真实入口打开，按键切到指定段。
fn module_frame(config: Config, width: u16, height: u16, section: usize) -> (Buffer, Rect) {
    let mut app = App::new(config).expect("app builds");
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal
        .draw(|frame| ui::draw(frame, &mut app))
        .expect("draw");
    let content = app.content;
    app.open_settings();
    support::pump_until_ready(&mut app, &mut terminal, &["settings"]);
    assert!(
        app.settings.is_none(),
        "settings module took the entry; note={:?}",
        app.note
    );
    for _ in 0..section {
        key(&mut app, &mut terminal, content, KeyCode::Tab);
    }
    (terminal.backend().buffer().clone(), content)
}

/// 逐格比较字符；返回差异格数并打印第一处差异。
fn compare(label: &str, builtin: &Buffer, module: &Buffer, area: Rect) -> usize {
    let mut differences = 0;
    let mut first: Option<String> = None;
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
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
    let total = usize::from(area.width) * usize::from(area.height);
    println!("{label}: {differences} of {total} cells differ");
    if let Some(line) = first {
        println!("[{label}] first difference {line}");
        println!("[{label}] builtin:\n{}", region_text(builtin, area));
        println!("[{label}] module:\n{}", region_text(module, area));
    }
    differences
}

#[test]
fn settings_match_builtin_cell_for_cell() {
    let _core = support::IsolatedCore::new("m9c-settings");
    hc_tui::i18n::set_zh(false);
    let mut failures = 0;
    for (width, height) in [(80u16, 24u16), (120, 40)] {
        for values in [Values::Default, Values::Changed] {
            let root = unique_dir(&format!("parity-{width}x{height}"));
            let config = config_in(&root, values);
            install_settings(&config);
            for section in 0..4 {
                let (builtin, content) = builtin_frame(config.clone(), width, height, section);
                let (module, module_content) = module_frame(config.clone(), width, height, section);
                assert_eq!(content, module_content, "content rect must match");
                let state = match values {
                    Values::Default => "default",
                    Values::Changed => "changed",
                };
                let label = format!("settings {width}x{height} {state} section{section}");
                failures += compare(&label, &builtin, &module, content);
            }
            std::fs::remove_dir_all(root).ok();
        }
    }
    assert_eq!(
        failures, 0,
        "settings module differs from the builtin panel"
    );
}

/// 反例：模块少渲染一个项，同一段流程必须报出差异。
#[test]
fn missing_item_is_caught() {
    let _core = support::IsolatedCore::new("m9c-settings");
    hc_tui::i18n::set_zh(false);
    let root = unique_dir("negative");
    let config = config_in(&root, Values::Default);
    let broken = SETTINGS_TSX.replace(DROPPED_ITEM_OLD, DROPPED_ITEM_NEW);
    assert_ne!(broken, SETTINGS_TSX, "negative control marker is missing");
    write_module(
        &config,
        "settings",
        &broken,
        SETTINGS_MANIFEST,
        SETTINGS_NSH,
    );
    let (builtin, content) = builtin_frame(config.clone(), 80, 24, 0);
    let (module, _) = module_frame(config, 80, 24, 0);
    let differences = compare("settings negative control", &builtin, &module, content);
    assert!(
        differences > 0,
        "a missing row must show up as differing cells"
    );
    std::fs::remove_dir_all(root).ok();
}

/// 键盘：Tab 换段、j/k 移动、Enter 应用并落盘、Esc 与 q 关闭。
#[test]
fn keyboard_goes_through_the_module() {
    let _core = support::IsolatedCore::new("m9c-settings");
    hc_tui::i18n::set_zh(false);
    let root = unique_dir("keys");
    let config = config_in(&root, Values::Default);
    install_settings(&config);
    let config_path = config.config_path.clone();
    let mut app = App::new(config).expect("app builds");
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("terminal");
    terminal
        .draw(|frame| ui::draw(frame, &mut app))
        .expect("draw");
    let content = app.content;
    app.open_settings();
    support::pump_until_ready(&mut app, &mut terminal, &["settings"]);
    assert!(app.settings.is_none(), "module took the entry");

    key(&mut app, &mut terminal, content, KeyCode::Tab);
    let text = region_text(terminal.backend().buffer(), content);
    assert!(
        text.contains("symbols"),
        "Tab switched to indicators:\n{text}"
    );

    key(&mut app, &mut terminal, content, KeyCode::Char('j'));
    key(&mut app, &mut terminal, content, KeyCode::Char('k'));
    key(&mut app, &mut terminal, content, KeyCode::Char('j'));
    key(&mut app, &mut terminal, content, KeyCode::Enter);
    assert_eq!(
        app.config.indicators,
        Indicators::Symbols,
        "Enter applied symbols"
    );
    let written = std::fs::read_to_string(&config_path).expect("config written");
    assert!(
        written.contains("status_indicators = \"symbols\""),
        "config file holds the new value:\n{written}"
    );

    key(&mut app, &mut terminal, content, KeyCode::Esc);
    assert!(
        app.mods.surfaces.is_empty() && app.settings.is_none(),
        "Esc closed the module overlay"
    );

    prefix_settings(&mut app);
    support::pump_until_ready(&mut app, &mut terminal, &["settings"]);
    assert!(
        app.settings.is_none() && !app.mods.surfaces.is_empty(),
        "keys.settings opened the module overlay"
    );
    key(&mut app, &mut terminal, content, KeyCode::Char('q'));
    assert!(
        app.mods.surfaces.is_empty() && app.settings.is_none(),
        "q closed the module overlay"
    );
    std::fs::remove_dir_all(root).ok();
}

/// 鼠标：点击段标题切段，点击项应用设置。
#[test]
fn mouse_goes_through_the_module() {
    let _core = support::IsolatedCore::new("m9c-settings");
    hc_tui::i18n::set_zh(false);
    let root = unique_dir("mouse");
    let config = config_in(&root, Values::Default);
    install_settings(&config);
    let mut app = App::new(config).expect("app builds");
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("terminal");
    terminal
        .draw(|frame| ui::draw(frame, &mut app))
        .expect("draw");
    let content = app.content;
    app.open_settings();
    support::pump_until_ready(&mut app, &mut terminal, &["settings"]);

    let before = region_text(terminal.backend().buffer(), content);
    let (x, y) = find_cell(&before, content, "toasts").expect("toasts tab is drawn");
    click(&mut app, &mut terminal, content, x, y);
    let after = region_text(terminal.backend().buffer(), content);
    assert!(
        after.contains("system"),
        "tab click switched section:\n{after}"
    );

    let (x, y) = find_cell(&after, content, "system").expect("system row is drawn");
    click(&mut app, &mut terminal, content, x, y);
    assert_eq!(
        app.config.notifications,
        Delivery::System,
        "item click applied the setting"
    );
    std::fs::remove_dir_all(root).ok();
}

/// 在一帧文本里找第一个匹配子串的绝对坐标。
fn find_cell(text: &str, content: Rect, needle: &str) -> Option<(u16, u16)> {
    for (row, line) in text.lines().enumerate() {
        if let Some(column) = line.find(needle) {
            return Some((
                content.x + column as u16 + needle.chars().count() as u16 / 2,
                content.y + row as u16,
            ));
        }
    }
    None
}

/// 鼠标点击：按下与抬起连着投（中间不插帧，插帧会让引擎的提交换掉按下时
/// 的节点）。nsh 的按下/抬起命中在并发提交下偶发丢事件，这里最多重投三次；
/// 点击真的不生效时照样失败。
fn click(app: &mut App, terminal: &mut Terminal<TestBackend>, content: Rect, x: u16, y: u16) {
    let before = region_text(terminal.backend().buffer(), content);
    for _ in 0..3 {
        for kind in [
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
        ] {
            app.on_mouse(MouseEvent {
                kind,
                column: x,
                row: y,
                modifiers: KeyModifiers::NONE,
            });
        }
        if support::pump_until_repainted_within(app, terminal, content, &before, 200) {
            return;
        }
    }
    panic!(
        "click at ({x},{y}) never changed the panel; frame now:\n{}",
        region_text(terminal.backend().buffer(), content)
    );
}

/// `--no-mods` 与未安装时回落到内置浮层。
#[test]
fn no_mods_falls_back_to_the_builtin_panel() {
    let _core = support::IsolatedCore::new("m9c-settings");
    hc_tui::i18n::set_zh(false);
    let root = unique_dir("fallback");
    let mut config = config_in(&root, Values::Default);
    install_settings(&config);
    config.no_mods = true;
    let mut app = App::new(config).expect("app builds");
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("terminal");
    terminal
        .draw(|frame| ui::draw(frame, &mut app))
        .expect("draw");
    let content = app.content;
    let _ = app.execute_action("app.settings", serde_json::json!({}));
    assert!(app.settings.is_some(), "builtin overlay under --no-mods");
    assert!(app.mods.surfaces.is_empty(), "no module surface opened");
    prefix_settings(&mut app);
    terminal
        .draw(|frame| ui::draw(frame, &mut app))
        .expect("draw");
    println!(
        "--no-mods frame:\n{}",
        region_text(terminal.backend().buffer(), content)
    );

    let root = unique_dir("fallback-absent");
    let config = config_in(&root, Values::Default);
    let mut app = App::new(config).expect("app builds");
    let _ = app.execute_action("app.settings", serde_json::json!({}));
    assert!(
        app.settings.is_some(),
        "builtin overlay when the module is not installed"
    );
    assert!(app.mods.surfaces.is_empty(), "no module surface opened");
}

/// 主题动作真的把 `theme.changed` 投给声明了该事件的模块。
#[test]
fn theme_changed_reaches_modules() {
    let _core = support::IsolatedCore::new("m9c-settings");
    hc_tui::i18n::set_zh(false);
    let root = unique_dir("theme");
    let config = config_in(&root, Values::Default);
    write_module(
        &config,
        "theme-probe",
        THEME_PROBE_TSX,
        "[mod]\nid = \"theme-probe\"\nplace = \"none\"\nhost_api = 1\n\n[grants]\nhost = [\"state\"]\nevents = [\"theme.changed\"]\n",
        r#"{"name":"ThemeProbe","id":"dev.hctui.mods.theme-probe","version":"1.0.0","build":1,"capabilities":{"host":["state"]}}"#,
    );
    let mut app = App::new(config).expect("app builds");
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("terminal");
    terminal
        .draw(|frame| ui::draw(frame, &mut app))
        .expect("draw");
    app.mods_open(
        "theme-probe",
        hc_tui::mods::SurfacePlace::Dialog,
        serde_json::json!({}),
        Some((30, 8)),
        None,
        None,
    )
    .expect("probe opens");
    let content = app.content;
    support::pump_until_ready(&mut app, &mut terminal, &["theme-probe"]);

    let before = region_text(terminal.backend().buffer(), content);
    app.execute_action("app.theme", serde_json::json!({ "name": "nord" }))
        .expect("theme applies");
    support::pump_until_repainted(&mut app, &mut terminal, content, &before);
    let text = region_text(terminal.backend().buffer(), content);
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        println!("probe frame: {}", line.trim_end());
    }
    assert!(
        text.contains("theme.changed:nord"),
        "module heard theme.changed"
    );
    assert_eq!(app.config.theme.name, "nord", "theme applied");
    std::fs::remove_dir_all(root).ok();
}

/// 证据用：把配置写到 `M9C_EVIDENCE_DIR`，用模块应用一个主题后保留配置，
/// 供 `hc-tui config check` 前后对照。跑法：
/// `M9C_EVIDENCE_DIR=/tmp/m9c-evidence cargo test --test m9c_settings_parity keep_config -- --ignored --nocapture`
#[test]
#[ignore]
fn keep_config_after_module_theme_change() {
    let _core = support::IsolatedCore::new("m9c-settings");
    hc_tui::i18n::set_zh(false);
    let dir = std::env::var("M9C_EVIDENCE_DIR")
        .map(PathBuf::from)
        .expect("M9C_EVIDENCE_DIR is set");
    std::fs::create_dir_all(&dir).expect("evidence dir");
    let config = config_in(&dir, Values::Default);
    install_settings(&config);
    let config_path = config.config_path.clone();
    let mut app = App::new(config).expect("app builds");
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("terminal");
    terminal
        .draw(|frame| ui::draw(frame, &mut app))
        .expect("draw");
    let content = app.content;
    app.open_settings();
    support::pump_until_ready(&mut app, &mut terminal, &["settings"]);
    key(&mut app, &mut terminal, content, KeyCode::Char('j'));
    key(&mut app, &mut terminal, content, KeyCode::Enter);
    println!("config_path={}", config_path.display());
    println!("theme={}", app.config.theme.name);
    println!(
        "file:\n{}",
        std::fs::read_to_string(&config_path).expect("config written")
    );
}
