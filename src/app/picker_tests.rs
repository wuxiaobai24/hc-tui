//! Picker 归一与对拍：九种来源经模块打开，模块与内置实现逐格一致。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui_core::{backend::TestBackend, layout::Rect, terminal::Terminal};
use serde_json::json;

use super::*;
use crate::config::{Binding, Config};
use crate::plugin::registry::EnablePolicy;

type Opener = Box<dyn Fn(&mut App)>;
type ActionBuilder = Box<dyn Fn(&mut App) -> Value>;

fn temp_root(tag: &str) -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!("hctui-m9d-{tag}-{}-{unique}", std::process::id()))
}

fn offline_app(tag: &str) -> App {
    crate::i18n::set_zh(false);
    let root = temp_root(tag);
    let mut app = App::offline(Config {
        restore: false,
        prompt_new_tab: false,
        config_path: root.join("config").join("config.toml"),
        state_path: root.join("state").join("state.json"),
        ..Config::default()
    })
    .unwrap();
    app.config.theme = crate::theme::builtin("catppuccin").unwrap();
    app
}

fn picker_source() -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("examples")
            .join("mods")
            .join("picker")
            .join("App.tsx"),
    )
    .unwrap()
}

fn write_picker(app: &App, source: &str) {
    let code = nsh_bundler::compile_module("App.tsx", source, &[]).unwrap();
    let dir = app.config.mods_dir().join("picker");
    std::fs::create_dir_all(dir.join("dist")).unwrap();
    std::fs::write(dir.join("dist").join("bundle.js"), code).unwrap();
    std::fs::write(
        dir.join("dist").join("capabilities.json"),
        r#"{"host":["run","overlay","focus","state","theme","setProps"]}"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("hctui.toml"),
        "[mod]\nid = \"picker\"\nhost_api = 1\n\n[grants]\nhost = [\"run\",\"overlay\",\"focus\",\"state\",\"theme\",\"setProps\"]\n",
    )
    .unwrap();
}

fn install_plugin(config: &Config) {
    let package = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("packages")
        .join("git-board");
    let _ = crate::plugin::registry::install(config, &package, EnablePolicy::Enable);
}

fn data_app(tag: &str) -> App {
    let mut app = offline_app(tag);
    for name in ["hc-tui", "api", "web"] {
        app.execute_action("space.new", json!({"name": name}))
            .unwrap();
    }
    app
}

fn draw(app: &mut App, width: u16, height: u16) -> Terminal<TestBackend> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| crate::ui::draw(frame, app)).unwrap();
    terminal
}

fn rows(terminal: &Terminal<TestBackend>, rect: Rect) -> String {
    let buffer = terminal.backend().buffer();
    (rect.y..rect.bottom())
        .map(|y| {
            (rect.x..rect.right())
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn pump(app: &mut App, width: u16, height: u16, rect: Rect) -> String {
    let started = Instant::now();
    let deadline = started + Duration::from_secs(5);
    let mut last = String::new();
    let mut stable = 0;
    let mut last_change = Instant::now();
    loop {
        let terminal = draw(app, width, height);
        app.pump_mods();
        let screen = rows(&terminal, rect);
        stable = if screen == last { stable + 1 } else { 0 };
        last = screen;
        if stable == 0 {
            last_change = Instant::now();
        }
        if (stable >= 4 && last_change.elapsed() > Duration::from_millis(500))
            || Instant::now() > deadline
        {
            for _ in 0..6 {
                let terminal = draw(app, width, height);
                app.pump_mods();
                let _ = rows(&terminal, rect);
                std::thread::sleep(Duration::from_millis(15));
            }
            let terminal = draw(app, width, height);
            last = rows(&terminal, rect);
            for surface in &app.mods.surfaces {
                let errors = surface
                    .errors
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                let real = surface
                    .last_error
                    .as_ref()
                    .filter(|error| !error.starts_with("frame budget"));
                assert!(
                    errors.is_empty() && real.is_none(),
                    "mod {} errors: {:?} last_error={:?}",
                    surface.mod_id,
                    *errors,
                    surface.last_error
                );
            }
            return last;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn chooser_rect(app: &App, rows: usize, machines: bool) -> Rect {
    let content = app.content;
    let width = content.width.min(64);
    let row_height = if content.width <= 64 && content.height >= 12 {
        2
    } else {
        1
    };
    let wanted = (rows as u16) * row_height + 3 + 2 * u16::from(machines);
    let height = wanted.min(content.height.saturating_sub(2)).max(3);
    Rect::new(
        content.x + (content.width - width) / 2,
        content.y + (content.height - height) / 2,
        width,
        height,
    )
}

fn goto_rect(app: &App) -> Rect {
    let screen = app.screen;
    Rect::new(
        screen.x + 2,
        screen.y + 1,
        screen.width.saturating_sub(4),
        screen.height.saturating_sub(2),
    )
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn keys(app: &mut App, list: &[KeyEvent]) {
    for event in list {
        app.on_key(*event);
    }
}

/// 同一个来源：内置侧禁用模块，模块侧装上 picker，按键序列与矩形相同。
fn compare(
    tag: &str,
    width: u16,
    height: u16,
    source: &str,
    rect: impl Fn(&App) -> Rect,
    open: impl Fn(&mut App) -> bool,
    events: &[KeyEvent],
) {
    let mut builtin_app = data_app(&format!("{tag}-b"));
    let _ = draw(&mut builtin_app, width, height);
    builtin_app.config.no_mods = true;
    open(&mut builtin_app);
    keys(&mut builtin_app, events);
    let area = rect(&builtin_app);
    let builtin = rows(&draw(&mut builtin_app, width, height), area);

    let mut mod_app = data_app(&format!("{tag}-m"));
    write_picker(&mod_app, source);
    let _ = draw(&mut mod_app, width, height);
    assert!(open(&mut mod_app), "picker module did not open for {tag}");
    let area = rect(&mod_app);
    let _ = pump(&mut mod_app, width, height, area);
    keys(&mut mod_app, events);
    let module = pump(&mut mod_app, width, height, area);
    assert_eq!(builtin, module, "frame differs for {tag}");
}

#[test]
fn chooser_root_parity() {
    compare(
        "root",
        80,
        24,
        &picker_source(),
        |app| chooser_rect(app, 4, false),
        |app| app.open_actions_picker_group("root"),
        &[],
    );
}

#[test]
fn chooser_scrolled_parity() {
    // 十七项的子菜单，选中第 13 项：窗口必须跟着滚。
    let mut down: Vec<KeyEvent> = (0..12).map(|_| key(KeyCode::Down)).collect();
    down.push(key(KeyCode::Char('j')));
    compare(
        "scroll",
        80,
        24,
        &picker_source(),
        |app| chooser_rect(app, 18, false),
        |app| app.open_actions_picker_group("panes"),
        &down,
    );
}

#[test]
fn chooser_machines_parity() {
    let source = picker_source();
    let mut builtin_app = data_app("machines-b");
    let _ = draw(&mut builtin_app, 80, 24);
    builtin_app.config.no_mods = true;
    builtin_app.machines = vec![
        MachineInfo {
            name: "build".into(),
            target: "dev@build".into(),
            state: "connected".into(),
            host: "build".into(),
        },
        MachineInfo {
            name: "gpu".into(),
            target: "dev@gpu".into(),
            state: "offline".into(),
            host: "gpu".into(),
        },
    ];
    builtin_app.open_machines_picker();
    keys(&mut builtin_app, &[key(KeyCode::Down)]);
    let rect = chooser_rect(&builtin_app, 2, true);
    let builtin = rows(&draw(&mut builtin_app, 80, 24), rect);

    let mut app = data_app("machines-m");
    write_picker(&app, &source);
    let _ = draw(&mut app, 80, 24);
    app.machines = builtin_app.machines.clone();
    assert!(app.open_machines_picker());
    let _ = pump(&mut app, 80, 24, rect);
    keys(&mut app, &[key(KeyCode::Down)]);
    let module = pump(&mut app, 80, 24, rect);
    assert_eq!(builtin, module, "machine chooser differs");
}

#[test]
fn goto_empty_parity() {
    compare(
        "goto-empty",
        80,
        24,
        &picker_source(),
        goto_rect,
        |app| {
            app.config.keys.goto = Binding::parse("ctrl+g").unwrap();
            app.on_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL));
            true
        },
        &[],
    );
}

#[test]
fn goto_filtered_parity() {
    compare(
        "goto-filtered",
        80,
        24,
        &picker_source(),
        goto_rect,
        |app| {
            app.config.keys.goto = Binding::parse("ctrl+g").unwrap();
            app.on_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL));
            true
        },
        &[
            key(KeyCode::Char('w')),
            key(KeyCode::Char('e')),
            key(KeyCode::Char('b')),
        ],
    );
}

#[test]
fn goto_selected_non_first_parity() {
    compare(
        "goto-selected",
        120,
        40,
        &picker_source(),
        goto_rect,
        |app| {
            app.config.keys.goto = Binding::parse("ctrl+g").unwrap();
            app.on_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL));
            true
        },
        &[
            key(KeyCode::Down),
            key(KeyCode::Down),
            key(KeyCode::Down),
            key(KeyCode::Down),
        ],
    );
}

#[test]
fn palette_parity() {
    compare(
        "palette",
        80,
        24,
        &picker_source(),
        goto_rect,
        |app| {
            app.execute_action("app.palette", json!({})).unwrap();
            true
        },
        &[],
    );
}

#[test]
fn palette_filtered_parity() {
    compare(
        "palette-filtered",
        80,
        24,
        &picker_source(),
        goto_rect,
        |app| {
            app.execute_action("app.palette", json!({})).unwrap();
            true
        },
        &[
            key(KeyCode::Char('l')),
            key(KeyCode::Char('a')),
            key(KeyCode::Char('y')),
            key(KeyCode::Char('o')),
            key(KeyCode::Char('u')),
            key(KeyCode::Char('t')),
        ],
    );
}

#[test]
fn wheel_moves_selection_like_builtin() {
    let source = picker_source();
    let rect = {
        let mut app = data_app("wheel-b");
        let _ = draw(&mut app, 80, 24);
        chooser_rect(&app, 18, false)
    };
    let wheel = MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: rect.x + rect.width / 2,
        row: rect.y + 2,
        modifiers: KeyModifiers::NONE,
    };
    let mut builtin_app = data_app("wheel-b");
    let _ = draw(&mut builtin_app, 80, 24);
    builtin_app.config.no_mods = true;
    builtin_app.open_actions_picker_group("panes");
    builtin_app.on_mouse(wheel);
    let builtin = rows(&draw(&mut builtin_app, 80, 24), rect);

    let mut app = data_app("wheel-m");
    write_picker(&app, &source);
    let _ = draw(&mut app, 80, 24);
    assert!(app.open_actions_picker_group("panes"));
    let _ = pump(&mut app, 80, 24, rect);
    app.on_mouse(wheel);
    let module = pump(&mut app, 80, 24, rect);
    assert_eq!(builtin, module, "wheel translation differs");
}

#[test]
fn counterexample_skipping_a_row_breaks_parity() {
    let source = picker_source().replace("const DEBUG_SKIP_ROW = 0;", "const DEBUG_SKIP_ROW = 1;");
    let rect = {
        let mut app = data_app("counter-b");
        let _ = draw(&mut app, 80, 24);
        chooser_rect(&app, 4, false)
    };
    let mut builtin_app = data_app("counter-b");
    let _ = draw(&mut builtin_app, 80, 24);
    builtin_app.config.no_mods = true;
    builtin_app.open_actions_picker_group("root");
    let builtin = rows(&draw(&mut builtin_app, 80, 24), rect);

    let mut app = data_app("counter-m");
    write_picker(&app, &source);
    let _ = draw(&mut app, 80, 24);
    assert!(app.open_actions_picker_group("root"));
    let module = pump(&mut app, 80, 24, rect);
    assert_ne!(builtin, module, "dropping a row must break parity");
}

#[test]
fn counterexample_shifted_column_breaks_parity() {
    let source =
        picker_source().replace("const DEBUG_SHIFT_COL = 0;", "const DEBUG_SHIFT_COL = 1;");
    let rect = {
        let mut app = data_app("counter2-b");
        let _ = draw(&mut app, 80, 24);
        chooser_rect(&app, 4, false)
    };
    let mut builtin_app = data_app("counter2-b");
    let _ = draw(&mut builtin_app, 80, 24);
    builtin_app.config.no_mods = true;
    builtin_app.open_actions_picker_group("root");
    let builtin = rows(&draw(&mut builtin_app, 80, 24), rect);

    let mut app = data_app("counter2-m");
    write_picker(&app, &source);
    let _ = draw(&mut app, 80, 24);
    assert!(app.open_actions_picker_group("root"));
    let module = pump(&mut app, 80, 24, rect);
    assert_ne!(builtin, module, "a one-column shift must break parity");
}

fn module_surface(app: &App) -> Option<&crate::mods::ModSurface> {
    app.mods
        .surfaces
        .iter()
        .find(|surface| surface.mod_id == "picker")
}

#[test]
fn nine_sources_open_through_the_module_and_run_their_rows() {
    let source = picker_source();
    let mut app = data_app("sources");
    install_plugin(&app.config);
    app.plugins = crate::plugin::registry::Registry::load(&app.config);
    write_picker(&app, &source);
    app.mod_templates = vec![
        json!({"name": "dev", "title": "Dev", "description": "run the dev server"}),
        json!({"name": "test", "title": "Test", "description": "run the test suite"}),
    ];
    app.machines = vec![MachineInfo {
        name: "build".into(),
        target: "dev@build".into(),
        state: "connected".into(),
        host: "build".into(),
    }];
    let _ = draw(&mut app, 80, 24);

    // 1 动作菜单（含子菜单与 Back）。
    assert!(app.open_actions_picker_group("root"));
    assert!(module_surface(&app).is_some());
    app.pump_mods();
    assert!(app.open_actions_picker_group("panes"));
    app.execute_action("picker.menu", json!({"group": "root"}))
        .unwrap();
    assert!(app.chooser.is_none());

    // 2 工作区选择。
    assert!(app.open_workspace_picker());
    app.execute_action("picker.key", json!({"name": "workspace_picker"}))
        .unwrap();
    let second = app.workspaces[1].id;
    app.execute_action("space.select", json!({"id": second.to_string()}))
        .unwrap();
    assert_eq!(app.active_ws, 1, "workspace picker row runs space.select");

    // 3 移动 Pane。
    assert!(app.open_move_pane_picker());
    app.execute_action("interrupt", json!({})).ok();

    // 4 Resize。
    assert!(app.open_resize_picker());
    app.execute_action("picker.resize", json!({"key": "l"}))
        .unwrap();
    app.execute_action("picker.resize", json!({"key": "done"}))
        .unwrap();
    assert!(!app.resize_mode);

    // 5 Swap Pane：切分后换位。
    app.execute_action("pane.split", json!({"direction": "right"}))
        .unwrap();
    assert!(app.open_swap_picker());

    // 6 终端菜单（含插件动作的真实数据）。
    assert!(app.open_terminal_picker_menu("t1"));

    // 7 模板。
    assert!(app.open_templates_picker());
    let tabs = app.workspace().map(|ws| ws.tabs.len()).unwrap_or(0);
    app.execute_action("picker.template", json!({"argv": ["dev"]}))
        .unwrap();
    assert_eq!(
        app.workspace().map(|ws| ws.tabs.len()).unwrap_or(0),
        tabs + 1,
        "template row opens a tab"
    );

    // 8 机器。
    assert!(app.open_machines_picker());
    app.execute_action(
        "picker.machine",
        json!({"op": "connect", "machine": "build"}),
    )
    .unwrap();

    // 9 授予确认。
    app.mods
        .request_grants("demo", &["run".to_string(), "notify".to_string()]);
    assert!(app.open_grants_picker("demo"));
    app.execute_action("picker.grant", json!({"mod": "demo", "grant": "run"}))
        .unwrap();

    // 插件动作行来自真实包：global 上下文进 palette，pane 上下文进终端菜单。
    let palette = app.picker_goto_rows(true);
    assert!(
        palette
            .iter()
            .any(|row| row.label == "dev.hctui.pkg.git-board.summary"),
        "plugin action missing from palette rows"
    );
    assert!(
        app.plugins
            .actions()
            .iter()
            .any(|action| action.contexts.iter().any(|context| context == "pane")),
        "plugin action with pane context missing"
    );
}

#[test]
fn state_snapshot_carries_the_picker_data_plane() {
    let source = picker_source();
    let mut app = data_app("data-plane");
    install_plugin(&app.config);
    app.plugins = crate::plugin::registry::Registry::load(&app.config);
    write_picker(&app, &source);
    app.mod_templates = vec![json!({
        "name": "dev",
        "title": "Dev",
        "description": "run the dev server"
    })];
    app.machines = vec![MachineInfo {
        name: "build".into(),
        target: "dev@build".into(),
        state: "connected".into(),
        host: "build".into(),
    }];
    let _ = draw(&mut app, 80, 24);

    let snapshot =
        app.mod_state_snapshot(&["commands", "machineInfo", "templates", "panes", "machines"]);
    let commands = snapshot["commands"].as_array().unwrap();
    assert!(commands.iter().any(|row| row["name"] == "machine.rename"));
    let plugin = commands
        .iter()
        .find(|row| row["name"] == "dev.hctui.pkg.git-board.summary")
        .expect("plugin action in commands");
    assert!(plugin["contexts"]
        .as_array()
        .is_some_and(|list| !list.is_empty()));
    assert!(plugin["args"].is_object(), "plugin args schema: {plugin}");
    assert!(!plugin["description"].as_str().unwrap_or("").is_empty());

    let machine = &snapshot["machineInfo"][0];
    assert_eq!(machine["name"], "build");
    assert_eq!(machine["state"], "connected");
    assert_eq!(machine["address"], "dev@build");
    assert_eq!(machine["host"], "build");

    assert_eq!(snapshot["templates"][0]["name"], "dev");

    let panes = snapshot["panes"].as_array().unwrap();
    assert!(!panes.is_empty());
    let pane = &panes[0];
    println!("plugin action: {plugin}");
    println!("templates: {}", snapshot["templates"]);
    println!("machineInfo: {}", snapshot["machineInfo"]);
    println!("pane: {pane}");
    let names: Vec<&str> = commands
        .iter()
        .filter(|row| {
            row["source"]
                .as_str()
                .is_some_and(|source| source.starts_with("plugin:"))
        })
        .filter_map(|row| row["name"].as_str())
        .collect();
    println!("plugin commands: {names:?}");
    for field in ["space", "tab", "label", "cwd", "live", "terminal"] {
        assert!(pane.get(field).is_some(), "panes missing {field}: {pane}");
    }
}

#[test]
fn no_mods_keeps_every_source_on_the_rust_path() {
    let source = picker_source();
    let mut app = data_app("no-mods");
    install_plugin(&app.config);
    app.plugins = crate::plugin::registry::Registry::load(&app.config);
    write_picker(&app, &source);
    app.config.no_mods = true;
    app.mod_templates = vec![json!({"name": "dev", "title": "Dev"})];
    app.machines = vec![MachineInfo {
        name: "build".into(),
        target: "dev@build".into(),
        state: "connected".into(),
        host: "build".into(),
    }];
    let _ = draw(&mut app, 80, 24);

    let cases: Vec<(&str, Opener)> = vec![
        (
            "actions",
            Box::new(|app: &mut App| {
                app.open_actions_picker_group("root");
            }),
        ),
        (
            "workspace",
            Box::new(|app: &mut App| {
                app.open_workspace_picker();
            }),
        ),
        (
            "move",
            Box::new(|app: &mut App| {
                app.open_move_pane_picker();
            }),
        ),
        (
            "resize",
            Box::new(|app: &mut App| {
                app.open_resize_picker();
            }),
        ),
        (
            "swap",
            Box::new(|app: &mut App| {
                app.open_swap_picker();
            }),
        ),
        (
            "terminal",
            Box::new(|app: &mut App| {
                app.open_terminal_picker_menu("t1");
            }),
        ),
        (
            "templates",
            Box::new(|app: &mut App| {
                app.open_templates_picker();
            }),
        ),
        (
            "machines",
            Box::new(|app: &mut App| {
                app.open_machines_picker();
            }),
        ),
        (
            "grants",
            Box::new(|app: &mut App| {
                app.mods.request_grants("demo", &["run".to_string()]);
                app.open_grants_picker("demo");
            }),
        ),
        (
            "goto",
            Box::new(|app: &mut App| {
                app.config.keys.goto = Binding::parse("ctrl+g").unwrap();
                app.on_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL));
            }),
        ),
        (
            "palette",
            Box::new(|app: &mut App| {
                app.execute_action("app.palette", json!({})).unwrap();
            }),
        ),
    ];
    for (name, open) in cases {
        open(&mut app);
        assert!(
            app.mods.surfaces.is_empty(),
            "{name} opened a module under --no-mods"
        );
        let fallback = if app.chooser.is_some() {
            "chooser"
        } else {
            "goto"
        };
        assert!(
            app.chooser.is_some() || app.goto.is_some(),
            "{name} did not fall back to the Rust picker"
        );
        println!(
            "{name}: surfaces=0 fallback={fallback} mods={}",
            app.mods.surfaces.len()
        );
        app.chooser = None;
        app.goto = None;
        app.palette = false;
    }
}

#[test]
#[ignore]
fn picker_sources_transcript() {
    let source = picker_source();
    let mut app = data_app("transcript");
    install_plugin(&app.config);
    app.plugins = crate::plugin::registry::Registry::load(&app.config);
    write_picker(&app, &source);
    app.mod_templates = vec![
        json!({"name": "dev", "title": "Dev", "description": "run the dev server"}),
        json!({"name": "test", "title": "Test", "description": "run the tests"}),
    ];
    app.machines = vec![MachineInfo {
        name: "build".into(),
        target: "dev@build".into(),
        state: "connected".into(),
        host: "build".into(),
    }];
    let _ = draw(&mut app, 80, 24);

    let cases: Vec<(&str, Opener, ActionBuilder)> = vec![
        (
            "动作菜单",
            Box::new(|app: &mut App| {
                app.open_actions_picker_group("root");
            }),
            Box::new(|_app| json!({"name": "picker.menu", "args": {"group": "panes"}})),
        ),
        (
            "工作区",
            Box::new(|app: &mut App| {
                app.open_workspace_picker();
            }),
            Box::new(
                |app| json!({"name": "space.select", "args": {"id": app.workspaces[1].id.to_string()}}),
            ),
        ),
        (
            "移动 Pane",
            Box::new(|app: &mut App| {
                app.open_move_pane_picker();
            }),
            Box::new(
                |app| json!({"name": "picker.move", "args": {"pane": app.focused(), "to": "new"}}),
            ),
        ),
        (
            "Resize",
            Box::new(|app: &mut App| {
                app.open_resize_picker();
            }),
            Box::new(|_app| json!({"name": "picker.resize", "args": {"key": "l"}})),
        ),
        (
            "Swap Pane",
            Box::new(|app: &mut App| {
                app.execute_action("pane.split", json!({"direction": "right"}))
                    .unwrap();
                app.open_swap_picker();
            }),
            Box::new(|app| {
                json!({"name": "pane.swap", "args": {
                    "pane": app.focused().to_string(),
                    "with": app.tab().into_iter().flat_map(|tab| tab.tree.panes())
                        .find(|pane| pane.0 != app.focused()).map(|pane| pane.0.to_string()).unwrap_or_default(),
                }})
            }),
        ),
        (
            "终端菜单",
            Box::new(|app: &mut App| {
                app.open_terminal_picker_menu("t1");
            }),
            Box::new(|_app| json!({"name": "terminal.peek", "args": {"terminal": "t1"}})),
        ),
        (
            "模板",
            Box::new(|app: &mut App| {
                app.open_templates_picker();
            }),
            Box::new(|_app| json!({"name": "picker.template", "args": {"argv": ["dev"]}})),
        ),
        (
            "机器",
            Box::new(|app: &mut App| {
                app.open_machines_picker();
            }),
            Box::new(
                |_app| json!({"name": "picker.machine", "args": {"op": "connect", "machine": "build"}}),
            ),
        ),
        (
            "授予确认",
            Box::new(|app: &mut App| {
                app.mods.request_grants("demo", &["run".to_string()]);
                app.open_grants_picker("demo");
            }),
            Box::new(
                |_app| json!({"name": "picker.grant", "args": {"mod": "demo", "grant": "run"}}),
            ),
        ),
    ];

    for (name, open, action) in cases {
        open(&mut app);
        let surface = app
            .mods
            .surfaces
            .iter()
            .find(|surface| surface.mod_id == "picker")
            .map(|surface| {
                let rows = surface
                    .props
                    .get("rows")
                    .and_then(Value::as_array)
                    .map(|rows| rows.len())
                    .unwrap_or(0);
                let title = surface
                    .props
                    .get("title")
                    .or_else(|| surface.props.get("navLeft"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                (rows, title, surface.area)
            });
        let Some((rows, title, area)) = surface else {
            println!("{name}: 模块未打开");
            continue;
        };
        let first = app
            .mods
            .surfaces
            .iter()
            .find(|surface| surface.mod_id == "picker")
            .and_then(|surface| surface.props.get("rows"))
            .and_then(Value::as_array)
            .and_then(|rows| rows.first())
            .and_then(|row| row.get("label"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let before = app.note.clone();
        let action = action(&mut app);
        let result = app.execute_action(
            action.get("name").and_then(Value::as_str).unwrap_or(""),
            action.get("args").cloned().unwrap_or_else(|| json!({})),
        );
        let outcome = match result {
            Ok(()) => {
                if app.note != before && !app.note.is_empty() {
                    app.note.clone()
                } else {
                    "accepted".to_string()
                }
            }
            Err(error) => format!("error: {error}"),
        };
        println!(
            "{name}: title={title:?} rows={rows} area={:?} first={first:?} -> {outcome}",
            area
        );
    }
}
