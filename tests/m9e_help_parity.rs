//! M9e help 对拍：模块键位浮层与内置浮层在同一个矩形里逐格比较字符格。
//!
//! 三个状态（无查询 / 有查询命中 / 查询无命中）× 80×24 与 120×40。
//! 另外：滚动条在首行 / 中段 / 末行三处的位置、键盘与鼠标走模块、反例必须
//! 报差异、`--no-mods` 与未安装时回落内置浮层。
//!
//! 颜色不是对拍对象：nsh TUI 把前景归到终端 16 色/Reset，内置实现写主题
//! RGB，逐格比色永远不相等（仓库既有 `scripts/mods-parity.sh` 同样只比字符格）。

use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use hc_tui::app::{App, HelpOverlay};
use hc_tui::config::Config;
use hc_tui::ui;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::Terminal;

const HELP_TSX: &str = include_str!("../examples/mods/help/App.tsx");
const HELP_MANIFEST: &str = include_str!("../examples/mods/help/hctui.toml");
const HELP_NSH: &str = include_str!("../examples/mods/help/nsh.json");
const HELP_CAPABILITIES: &str = r#"{"host":["closeOverlay"]}"#;

/// 故意漏掉每组第一行：用来证明对拍断言真的验到了键位行。
const DROPPED_ROW_OLD: &str = "for (const row of group.rows)";
const DROPPED_ROW_NEW: &str = "for (const row of group.rows.slice(1))";

fn unique_dir(tag: &str) -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!("hctui-m9e-{tag}-{}-{unique}", std::process::id()))
}

fn config_in(root: &Path) -> Config {
    Config {
        restore: false,
        prompt_new_tab: false,
        session: Some(format!("hcp-m9e-test-{}", std::process::id())),
        config_path: root.join("config").join("config.toml"),
        state_path: root.join("state").join("state.json"),
        theme: hc_tui::theme::builtin("catppuccin").expect("theme"),
        ..Config::default()
    }
}

fn write_module(config: &Config, tsx: &str) {
    let staging = unique_dir("stage-help");
    std::fs::create_dir_all(staging.join("dist")).expect("staging dist");
    std::fs::write(staging.join("hctui.toml"), HELP_MANIFEST).expect("manifest");
    std::fs::write(staging.join("nsh.json"), HELP_NSH).expect("nsh.json");
    std::fs::write(
        staging.join("dist").join("capabilities.json"),
        HELP_CAPABILITIES,
    )
    .expect("capabilities");
    let code = nsh_bundler::compile_module("App.tsx", tsx, &[]).expect("module compiles");
    std::fs::write(staging.join("dist").join("bundle.js"), code).expect("bundle");
    hc_tui::mods::store::install_local(config, &staging).expect("module installs");
    std::fs::remove_dir_all(staging).ok();
}

fn install_help(config: &Config) {
    let source = match std::env::var_os("M9E_NEGATIVE") {
        Some(_) => {
            let broken = HELP_TSX.replace(DROPPED_ROW_OLD, DROPPED_ROW_NEW);
            assert_ne!(broken, HELP_TSX, "negative control marker is missing");
            broken
        }
        None => HELP_TSX.to_string(),
    };
    write_module(config, &source);
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

/// 内置浮层的帧：打开帮助、停在指定查询与选中行。
fn builtin_frame(
    config: Config,
    width: u16,
    height: u16,
    query: &str,
    selected: usize,
) -> (Buffer, Rect) {
    let mut app = App::new(config).expect("app builds");
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal
        .draw(|frame| ui::draw(frame, &mut app))
        .expect("draw");
    let content = app.content;
    app.help = Some(HelpOverlay {
        query: query.to_string(),
        selected,
        scroll: 0,
    });
    terminal
        .draw(|frame| ui::draw(frame, &mut app))
        .expect("draw");
    (terminal.backend().buffer().clone(), content)
}

/// 让模块表面出帧，直到内容区连续几帧不再变化且出现 `needle`。
fn pump_until_stable(
    app: &mut App,
    terminal: &mut Terminal<TestBackend>,
    content: Rect,
    needle: &str,
) {
    let mut previous = String::new();
    let mut stable = 0;
    for round in 0..600 {
        terminal.draw(|frame| ui::draw(frame, app)).expect("draw");
        app.pump_mods();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let text = region_text(terminal.backend().buffer(), content);
        stable = if text == previous { stable + 1 } else { 0 };
        previous = text.clone();
        if round > 8 && stable >= 2 && text.contains(needle) {
            return;
        }
    }
}

/// 开着模块浮层的应用；按键与鼠标都走真实入口。
struct Harness {
    app: App,
    terminal: Terminal<TestBackend>,
    content: Rect,
}

impl Harness {
    fn open(config: Config, width: u16, height: u16) -> Harness {
        let mut app = App::new(config).expect("app builds");
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        terminal
            .draw(|frame| ui::draw(frame, &mut app))
            .expect("draw");
        let content = app.content;
        app.open_help();
        let mut harness = Harness {
            app,
            terminal,
            content,
        };
        harness.pump("keybinds");
        assert!(
            harness.app.help.is_none(),
            "help module took the entry; note={:?}",
            harness.app.note
        );
        assert!(
            !harness.app.mods.surfaces.is_empty(),
            "help module opened a surface"
        );
        harness
    }

    fn pump(&mut self, needle: &str) {
        pump_until_stable(&mut self.app, &mut self.terminal, self.content, needle);
    }

    fn key(&mut self, code: KeyCode) {
        self.key_with(code, KeyModifiers::NONE);
    }

    fn key_with(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        self.app.on_key(KeyEvent::new(code, modifiers));
        self.pump("");
    }

    fn text(&self) -> String {
        region_text(self.terminal.backend().buffer(), self.content)
    }

    fn buffer(&self) -> Buffer {
        self.terminal.backend().buffer().clone()
    }

    fn closed(&self) -> bool {
        self.app.help.is_none() && self.app.mods.surfaces.is_empty()
    }
}

/// 模块浮层的帧：真实入口打开，按查询键入字符，再按 `selected` 次 `j`。
fn module_frame(
    config: Config,
    width: u16,
    height: u16,
    query: &str,
    selected: usize,
) -> (Buffer, Rect) {
    let mut harness = Harness::open(config, width, height);
    for ch in query.chars() {
        harness.key(KeyCode::Char(ch));
    }
    if !query.is_empty() {
        harness.pump(query);
    }
    for _ in 0..selected {
        harness.key(KeyCode::Char('j'));
    }
    (harness.buffer(), harness.content)
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

/// 正文区最后一列的字符（右侧滚动条列；上边框、标题、查询与分隔线不算）。
fn scrollbar_column(buffer: &Buffer, area: Rect) -> String {
    let x = area.right().saturating_sub(2);
    (area.y + 4..area.bottom().saturating_sub(1))
        .map(|y| buffer[(x, y)].symbol())
        .collect()
}

#[test]
fn help_matches_builtin_cell_for_cell() {
    hc_tui::i18n::set_zh(false);
    let mut failures = 0;
    let states = [("empty", ""), ("hit", "zoom"), ("miss", "zzz")];
    for (width, height) in [(80u16, 24u16), (120, 40)] {
        let root = unique_dir(&format!("parity-{width}x{height}"));
        let config = config_in(&root);
        install_help(&config);
        for (name, query) in states {
            let (builtin, content) = builtin_frame(config.clone(), width, height, query, 0);
            let (module, module_content) = module_frame(config.clone(), width, height, query, 0);
            assert_eq!(content, module_content, "content rect must match");
            failures += compare(
                &format!("help {width}x{height} {name}"),
                &builtin,
                &module,
                content,
            );
        }
        std::fs::remove_dir_all(root).ok();
    }
    assert_eq!(failures, 0, "help module differs from the builtin panel");
}

/// 滚动条位置：首行 / 中段 / 末行各比一次，并把两侧帧贴出来。
#[test]
fn scroll_positions_match() {
    hc_tui::i18n::set_zh(false);
    let root = unique_dir("scroll");
    let config = config_in(&root);
    install_help(&config);
    let rows = App::new(config.clone())
        .expect("app builds")
        .help_rows()
        .len();
    let positions = [("first", 0usize), ("middle", rows / 2), ("last", rows - 1)];
    let mut failures = 0;
    for (name, selected) in positions {
        let (builtin, content) = builtin_frame(config.clone(), 80, 24, "", selected);
        let (module, module_content) = module_frame(config.clone(), 80, 24, "", selected);
        assert_eq!(content, module_content, "content rect must match");
        failures += compare(&format!("help scroll {name}"), &builtin, &module, content);
        println!("[{name}] selected={selected}");
        println!(
            "[{name}] builtin frame:\n{}",
            region_text(&builtin, content)
        );
        println!("[{name}] module frame:\n{}", region_text(&module, content));
        println!(
            "[{name}] builtin scrollbar: {:?}",
            scrollbar_column(&builtin, content)
        );
        println!(
            "[{name}] module scrollbar:  {:?}",
            scrollbar_column(&module, content)
        );
    }
    std::fs::remove_dir_all(root).ok();
    assert_eq!(failures, 0, "scroll position differs");
}

/// 反例：模块少画一组的第一行键位，同一流程必须报出差异。
#[test]
fn missing_row_is_caught() {
    hc_tui::i18n::set_zh(false);
    let root = unique_dir("negative");
    let config = config_in(&root);
    let broken = HELP_TSX.replace(DROPPED_ROW_OLD, DROPPED_ROW_NEW);
    assert_ne!(broken, HELP_TSX, "negative control marker is missing");
    write_module(&config, &broken);
    let (builtin, content) = builtin_frame(config.clone(), 80, 24, "", 0);
    let (module, _) = module_frame(config, 80, 24, "", 0);
    let differences = compare("help negative control", &builtin, &module, content);
    assert!(
        differences > 0,
        "a missing row must show up as differing cells"
    );
    std::fs::remove_dir_all(root).ok();
}

/// 键盘：查询输入、退格、移动键都走模块；`Esc`/`Enter`/空查询的 `q` 关闭。
#[test]
fn keyboard_goes_through_the_module() {
    hc_tui::i18n::set_zh(false);
    let root = unique_dir("keys");
    let config = config_in(&root);
    install_help(&config);
    let mut harness = Harness::open(config.clone(), 80, 24);

    for ch in "zoom".chars() {
        harness.key(KeyCode::Char(ch));
    }
    harness.pump("zoom");
    let text = harness.text();
    assert!(
        text.contains("zoom"),
        "query typed into the module:\n{text}"
    );
    assert!(
        text.contains(" matches"),
        "命中时插入 matches 分组:\n{text}"
    );

    harness.key_with(KeyCode::Char('A'), KeyModifiers::SHIFT);
    harness.pump("zoomA");
    assert!(
        harness.text().contains("zoomA"),
        "Shift 字符照原样进查询:\n{}",
        harness.text()
    );

    for _ in 0.."zoomA".len() {
        harness.key(KeyCode::Backspace);
    }
    harness.pump("press / to filter");
    assert!(
        harness.text().contains("press / to filter"),
        "退格清空查询并回到提示行:\n{}",
        harness.text()
    );

    // j/j/j + PgDn + k + PgUp = 选中第 2 行，与内置同状态逐格一致。
    for _ in 0..3 {
        harness.key(KeyCode::Char('j'));
    }
    harness.key(KeyCode::PageDown);
    harness.key(KeyCode::Char('k'));
    harness.key(KeyCode::PageUp);
    let (builtin, content) = builtin_frame(config.clone(), 80, 24, "", 2);
    assert_eq!(content, harness.content);
    let differences = compare(
        "help keyboard selection",
        &builtin,
        &harness.buffer(),
        content,
    );
    assert_eq!(differences, 0, "模块里的移动键与内置同位置");

    harness.key(KeyCode::Esc);
    assert!(harness.closed(), "Esc closed the module overlay");

    // `keys.help`（prefix+?）走的也是模块。
    harness
        .app
        .on_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
    harness
        .app
        .on_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
    harness.pump("keybinds");
    assert!(
        harness.app.help.is_none() && !harness.app.mods.surfaces.is_empty(),
        "keys.help opened the module overlay"
    );
    harness.key(KeyCode::Char('q'));
    assert!(harness.closed(), "q closed with an empty query");

    harness
        .app
        .execute_action("app.help", serde_json::json!({}))
        .expect("app.help dispatches");
    harness.pump("keybinds");
    harness.key(KeyCode::Char('x'));
    harness.pump("x");
    harness.key(KeyCode::Char('q'));
    harness.pump("xq");
    assert!(
        harness.text().contains("xq"),
        "非空查询下 q 是普通字符:\n{}",
        harness.text()
    );
    harness.key(KeyCode::Enter);
    assert!(harness.closed(), "Enter closed the module overlay");
    std::fs::remove_dir_all(root).ok();
}

/// 鼠标：右上关闭按钮走模块的 `onPress`。
#[test]
fn mouse_close_click_goes_through_the_module() {
    hc_tui::i18n::set_zh(false);
    let root = unique_dir("mouse");
    let config = config_in(&root);
    install_help(&config);
    let mut harness = Harness::open(config, 80, 24);
    let x = harness.content.right() - 3;
    let y = harness.content.y;
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
        harness.pump("");
    }
    assert!(harness.closed(), "[x] click closed the module overlay");
    std::fs::remove_dir_all(root).ok();
}

/// `--no-mods` 与未安装时回落到内置浮层。
#[test]
fn no_mods_falls_back_to_the_builtin_panel() {
    hc_tui::i18n::set_zh(false);
    let root = unique_dir("fallback");
    let mut config = config_in(&root);
    install_help(&config);
    config.no_mods = true;
    let mut app = App::new(config).expect("app builds");
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("terminal");
    terminal
        .draw(|frame| ui::draw(frame, &mut app))
        .expect("draw");
    let content = app.content;
    let _ = app.execute_action("app.help", serde_json::json!({}));
    assert!(app.help.is_some(), "builtin overlay under --no-mods");
    assert!(app.mods.surfaces.is_empty(), "no module surface opened");

    app.on_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
    app.on_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
    terminal
        .draw(|frame| ui::draw(frame, &mut app))
        .expect("draw");
    println!(
        "--no-mods frame:\n{}",
        region_text(terminal.backend().buffer(), content)
    );

    let root = unique_dir("fallback-absent");
    let config = config_in(&root);
    let mut app = App::new(config).expect("app builds");
    let _ = app.execute_action("app.help", serde_json::json!({}));
    assert!(
        app.help.is_some(),
        "builtin overlay when the module is not installed"
    );
    assert!(app.mods.surfaces.is_empty(), "no module surface opened");
}
