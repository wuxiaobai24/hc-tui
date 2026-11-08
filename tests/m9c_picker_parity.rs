//! M9d picker 对拍：两种形态各两个状态，模块与内置实现逐格一致；反例必须 FAIL。
//!
//! ```sh
//! HC_TUI_GRAPHICS=kitty cargo test --test m9c_picker_parity
//! ```

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use hc_tui::app::App;
use hc_tui::config::{Binding, Config};
use hc_tui::mods::ModSurface;
use ratatui::{backend::TestBackend, layout::Rect, Terminal};
use serde_json::json;

fn temp_root(tag: &str) -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!(
        "hctui-m9d-parity-{tag}-{}-{unique}",
        std::process::id()
    ))
}

fn app(tag: &str) -> App {
    let root = temp_root(tag);
    App::new(Config {
        restore: false,
        prompt_new_tab: false,
        sidebar: false,
        session: Some(format!("m9d-parity-{tag}")),
        config_path: root.join("config").join("config.toml"),
        state_path: root.join("state").join("state.json"),
        ..Config::default()
    })
    .unwrap()
}

fn data_app(tag: &str) -> App {
    let mut app = app(tag);
    for name in ["hc-tui", "api", "web"] {
        app.execute_action("space.new", json!({"name": name}))
            .unwrap();
    }
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

fn draw(app: &mut App, width: u16, height: u16) -> Terminal<TestBackend> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| hc_tui::ui::draw(frame, app)).unwrap();
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
    let deadline = started + Duration::from_secs(8);
    let mut last = String::new();
    let mut stable = 0;
    let mut changed = Instant::now();
    loop {
        let terminal = draw(app, width, height);
        app.pump_mods();
        let screen = rows(&terminal, rect);
        if screen == last {
            stable += 1;
        } else {
            stable = 0;
            changed = Instant::now();
        }
        last = screen;
        if (stable >= 4 && changed.elapsed() > Duration::from_millis(500))
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
                    "picker errors: {:?} last_error={:?}",
                    *errors,
                    surface.last_error
                );
            }
            return last;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn chooser_rect(app: &App, len: usize, machines: bool) -> Rect {
    let content = app.content;
    let width = content.width.min(64);
    let row_height = if content.width <= 64 && content.height >= 12 {
        2
    } else {
        1
    };
    let wanted = (len as u16) * row_height + 3 + 2 * u16::from(machines);
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

fn picker_surface(app: &App) -> Option<&ModSurface> {
    app.mods
        .surfaces
        .iter()
        .find(|surface| surface.mod_id == "picker")
}

/// 内置侧与模块侧各开一次，按键与矩形相同，逐格比较。
#[allow(clippy::too_many_arguments)]
fn compare(
    tag: &str,
    width: u16,
    height: u16,
    source: &str,
    len: usize,
    machines: bool,
    goto: bool,
    open: fn(&mut App),
    events: &[KeyEvent],
) {
    let mut builtin = data_app(&format!("{tag}-b"));
    let _ = draw(&mut builtin, width, height);
    builtin.config.no_mods = true;
    open(&mut builtin);
    for event in events {
        builtin.on_key(*event);
    }
    let rect = if goto {
        goto_rect(&builtin)
    } else {
        chooser_rect(&builtin, len, machines)
    };
    let expected = rows(&draw(&mut builtin, width, height), rect);

    let mut module = data_app(&format!("{tag}-m"));
    write_picker(&module, source);
    let _ = draw(&mut module, width, height);
    open(&mut module);
    assert!(
        picker_surface(&module).is_some(),
        "picker did not open: {tag}"
    );
    let _ = pump(&mut module, width, height, rect);
    for event in events {
        module.on_key(*event);
    }
    let actual = pump(&mut module, width, height, rect);
    assert_eq!(expected, actual, "module and builtin differ: {tag}");
}

fn open_root(app: &mut App) {
    app.open_actions_picker_group("root");
}

fn open_panes(app: &mut App) {
    app.open_actions_picker_group("panes");
}

fn open_goto(app: &mut App) {
    app.config.keys.goto = Binding::parse("ctrl+g").unwrap();
    app.on_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL));
}

fn open_palette(app: &mut App) {
    app.execute_action("app.palette", json!({})).unwrap();
}

#[test]
fn chooser_two_states_match() {
    let source = picker_source();
    // 空过滤：根菜单。
    compare("root", 80, 24, &source, 4, false, false, open_root, &[]);
    // 选中项非首行并滚动窗口：17 项的子菜单，走到第 13 项。
    let mut down: Vec<KeyEvent> = (0..12).map(|_| key(KeyCode::Down)).collect();
    down.push(key(KeyCode::Char('j')));
    compare(
        "scroll", 80, 24, &source, 18, false, false, open_panes, &down,
    );
    compare(
        "machines",
        80,
        24,
        &source,
        2,
        true,
        false,
        |app| {
            app.machines = vec![
                hc_tui::app::MachineInfo {
                    name: "build".into(),
                    target: "dev@build".into(),
                    state: "connected".into(),
                    host: "build".into(),
                },
                hc_tui::app::MachineInfo {
                    name: "gpu".into(),
                    target: "dev@gpu".into(),
                    state: "offline".into(),
                    host: "gpu".into(),
                },
            ];
            app.open_machines_picker();
        },
        &[key(KeyCode::Down)],
    );
}

#[test]
fn goto_two_states_match() {
    let source = picker_source();
    compare(
        "goto-empty",
        80,
        24,
        &source,
        0,
        false,
        true,
        open_goto,
        &[],
    );
    compare(
        "goto-filtered",
        80,
        24,
        &source,
        0,
        false,
        true,
        open_goto,
        &[
            key(KeyCode::Char('w')),
            key(KeyCode::Char('e')),
            key(KeyCode::Char('b')),
        ],
    );
}

#[test]
fn palette_two_states_match() {
    let source = picker_source();
    compare(
        "palette",
        80,
        24,
        &source,
        0,
        false,
        true,
        open_palette,
        &[],
    );
    compare(
        "palette-filtered",
        80,
        24,
        &source,
        0,
        false,
        true,
        open_palette,
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

/// 反例：故意少渲染一行，对拍必须 FAIL。
#[test]
fn counterexample_one_row_less_fails() {
    let source = picker_source().replace("const DEBUG_SKIP_ROW = 0;", "const DEBUG_SKIP_ROW = 1;");
    let mut builtin = data_app("counter-row-b");
    let _ = draw(&mut builtin, 80, 24);
    builtin.config.no_mods = true;
    open_root(&mut builtin);
    let rect = chooser_rect(&builtin, 4, false);
    let expected = rows(&draw(&mut builtin, 80, 24), rect);

    let mut module = data_app("counter-row-m");
    write_picker(&module, &source);
    let _ = draw(&mut module, 80, 24);
    open_root(&mut module);
    let actual = pump(&mut module, 80, 24, rect);
    assert_ne!(expected, actual, "dropping a row must break parity");
}

/// 反例：故意错位一列，对拍必须 FAIL。
#[test]
fn counterexample_one_column_off_fails() {
    let source =
        picker_source().replace("const DEBUG_SHIFT_COL = 0;", "const DEBUG_SHIFT_COL = 1;");
    let mut builtin = data_app("counter-col-b");
    let _ = draw(&mut builtin, 80, 24);
    builtin.config.no_mods = true;
    open_root(&mut builtin);
    let rect = chooser_rect(&builtin, 4, false);
    let expected = rows(&draw(&mut builtin, 80, 24), rect);

    let mut module = data_app("counter-col-m");
    write_picker(&module, &source);
    let _ = draw(&mut module, 80, 24);
    open_root(&mut module);
    let actual = pump(&mut module, 80, 24, rect);
    assert_ne!(expected, actual, "a one-column shift must break parity");
}
