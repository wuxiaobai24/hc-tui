//! M9b 交互：侧栏、Tab 栏、模式行的真实按键与鼠标序列。
//!
//! 三块的内容迁到模块后，输入仍由 Rust 侧处理，这里断言的是「同一套输入仍然
//! 改变同一份状态与同一帧」。构造方式沿用 `src/app/package_tests.rs`：临时配置
//! 目录 + `TestBackend` 帧循环。

use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use hc_tui::app::App;
use hc_tui::config::Config;
use hc_tui::model::{Tab, Workspace};
use hc_tui::theme::{self, Theme};
use hc_tui::ui;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

const THEMES: &str = include_str!("fixtures/m9b_themes.toml");
const SIDEBAR_TSX: &str = include_str!("../examples/mods/sidebar/App.tsx");
const SIDEBAR_MANIFEST: &str = include_str!("../examples/mods/sidebar/hctui.toml");
const SIDEBAR_NSH: &str = include_str!("../examples/mods/sidebar/nsh.json");
const TABS_TSX: &str = include_str!("../examples/mods/tabs/App.tsx");
const TABS_MANIFEST: &str = include_str!("../examples/mods/tabs/hctui.toml");
const TABS_NSH: &str = include_str!("../examples/mods/tabs/nsh.json");
const STATUS_TSX: &str = include_str!("../examples/mods/status/App.tsx");
const STATUS_MANIFEST: &str = include_str!("../examples/mods/status/hctui.toml");
const STATUS_NSH: &str = include_str!("../examples/mods/status/nsh.json");

#[derive(serde::Deserialize)]
struct ThemesFile {
    theme: Vec<ThemeEntry>,
}

#[derive(serde::Deserialize)]
struct ThemeEntry {
    name: String,
    tokens: std::collections::BTreeMap<String, String>,
}

fn parity_theme() -> Theme {
    let file: ThemesFile = toml::from_str(THEMES).expect("fixture themes parse");
    let entry = file.theme.into_iter().next().expect("one fixture theme");
    let mut theme = theme::builtin("default").expect("default theme");
    theme.name = entry.name;
    for (token, text) in entry.tokens {
        theme.set(
            &token,
            theme::parse_color(&text).expect("fixture color parses"),
        );
    }
    theme
}

fn unique_dir(tag: &str) -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!(
        "hctui-m9b-keys-{tag}-{}-{unique}",
        std::process::id()
    ))
}

fn config_in(root: &Path, theme: Theme) -> Config {
    Config {
        restore: false,
        prompt_new_tab: false,
        config_path: root.join("config").join("config.toml"),
        state_path: root.join("state").join("state.json"),
        theme,
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
            folder: Some("/nonexistent/m9b-one".to_string()),
            id: 1,
            label: "one".to_string(),
            tabs: vec![Tab::new(11, 101), Tab::new(12, 102)],
            active: 0,
        },
        Workspace {
            origin: None,
            machine: None,
            folder: Some("/nonexistent/m9b-two".to_string()),
            id: 2,
            label: "two".to_string(),
            tabs: vec![Tab::new(21, 201)],
            active: 0,
        },
    ];
    app.active_ws = 0;
}

fn app_with_regions() -> (App, PathBuf) {
    let root = unique_dir("app");
    let mut config = config_in(&root, parity_theme());
    config.zellij_keys = true;
    install_module(
        &config,
        "sidebar",
        SIDEBAR_MANIFEST,
        SIDEBAR_NSH,
        "{\"host\":[\"state\"]}",
        SIDEBAR_TSX,
    );
    install_module(
        &config,
        "tabs",
        TABS_MANIFEST,
        TABS_NSH,
        "{\"host\":[\"state\"]}",
        TABS_TSX,
    );
    install_module(
        &config,
        "status",
        STATUS_MANIFEST,
        STATUS_NSH,
        "{\"host\":[\"state\"]}",
        STATUS_TSX,
    );
    let mut app = App::new(config).expect("app builds");
    fixture_state(&mut app);
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("terminal");
    for _ in 0..60 {
        terminal
            .draw(|frame| ui::draw(frame, &mut app))
            .expect("draw");
        app.tick();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    (app, root)
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

fn key(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::empty()));
}

fn ctrl(app: &mut App, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::CONTROL));
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

#[test]
fn sidebar_selection_follows_prefix_and_workspace_picker() {
    let (mut app, root) = app_with_regions();
    assert_eq!(app.active_ws, 0);
    let before = screen(&mut app, 80, 24);
    assert!(before.contains("one"), "screen: {before}");

    // Ctrl+B 部署前缀，`w` 进工作区选择，`l` 选下一个 Space。
    ctrl(&mut app, KeyCode::Char('b'));
    key(&mut app, KeyCode::Char('w'));
    assert!(app.workspace_mode, "workspace picker must open");
    key(&mut app, KeyCode::Char('l'));
    assert_eq!(app.active_ws, 1);
    let after = screen(&mut app, 80, 24);
    let first = after
        .lines()
        .find(|line| line.starts_with(' ') && line.contains('W'))
        .unwrap_or_default()
        .to_string();
    assert!(
        first.contains("two") || after.contains("two"),
        "screen: {after}"
    );

    // `h` 回到前一个，`Esc` 退出选择模式。
    ctrl(&mut app, KeyCode::Char('b'));
    key(&mut app, KeyCode::Char('w'));
    key(&mut app, KeyCode::Char('h'));
    assert_eq!(app.active_ws, 0);
    key(&mut app, KeyCode::Esc);
    assert!(!app.workspace_mode);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn sidebar_rows_answer_mouse_clicks() {
    let (mut app, root) = app_with_regions();
    let before = screen(&mut app, 80, 24);
    assert!(before.lines().any(|line| line.contains("This computer")));
    assert_eq!(app.active_ws, 0);

    // 机器分组行（标题、空行之后）落在侧栏第 3 行：点击折叠，再点展开。
    click(&mut app, 2, 2);
    assert!(
        app.folded_agent_groups.contains("machine:local"),
        "groups: {:?}",
        app.folded_agent_groups
    );
    click(&mut app, 2, 2);
    assert!(!app.folded_agent_groups.contains("machine:local"));

    // 第二个 Space 的行：点击切过去。
    click(&mut app, 4, 4);
    assert_eq!(app.active_ws, 1);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn tab_bar_switches_and_adds_tabs_by_key() {
    let (mut app, root) = app_with_regions();
    let tabs = |app: &App| {
        app.workspace()
            .map(|workspace| (workspace.active, workspace.tabs.len()))
            .unwrap_or((0, 0))
    };
    assert_eq!(tabs(&app), (0, 2));
    ctrl(&mut app, KeyCode::Char('b'));
    key(&mut app, KeyCode::Char('n'));
    assert_eq!(tabs(&app).0, 1);
    ctrl(&mut app, KeyCode::Char('b'));
    key(&mut app, KeyCode::Char('p'));
    assert_eq!(tabs(&app).0, 0);
    let frame = screen(&mut app, 80, 24);
    assert!(
        frame.lines().next().is_some_and(|line| line.contains('1')),
        "screen: {frame}"
    );

    // zellij 的 Tab 模式：Ctrl+t 进入，l/h 左右切换，n 新建。
    ctrl(&mut app, KeyCode::Char('t'));
    assert_eq!(format!("{:?}", app.keyboard_mode), "Tab");
    key(&mut app, KeyCode::Char('l'));
    assert_eq!(tabs(&app).0, 1);
    key(&mut app, KeyCode::Char('h'));
    assert_eq!(tabs(&app).0, 0);
    // 仍在 Tab 模式里：`n` 新建一个 Tab 并退出模式。
    key(&mut app, KeyCode::Char('n'));
    assert_eq!(tabs(&app).1, 3);
    assert_eq!(tabs(&app).0, 2);
    assert_eq!(format!("{:?}", app.keyboard_mode), "Normal");

    // `+` 命中域在 Tab 栏行末尾之前：真实鼠标点击新建一个 Tab。
    let add = app.tab_add;
    assert!(add.width > 0, "tab add hit: {add:?}");
    click(&mut app, add.x, add.y);
    assert_eq!(tabs(&app).1, 4);
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn status_line_is_read_only() {
    let (mut app, root) = app_with_regions();
    let status_row = |app: &mut App| {
        let frame = screen(app, 80, 24);
        frame.lines().last().unwrap_or_default().to_string()
    };
    let before = status_row(&mut app);
    assert!(!before.trim().is_empty(), "status row: {before:?}");
    assert!(
        app.mod_regions.iter().any(|region| region.id == "status"),
        "status module must own the mode line"
    );
    for code in [
        KeyCode::Char('j'),
        KeyCode::Char('k'),
        KeyCode::Tab,
        KeyCode::Enter,
    ] {
        key(&mut app, code);
        let after = status_row(&mut app);
        assert_eq!(before, after, "status row changed by {code:?}: {after:?}");
    }
    std::fs::remove_dir_all(root).ok();
}
