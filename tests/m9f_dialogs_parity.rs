//! M9f dialogs 对拍：模块对话框与内置 Prompt / 参数表单逐格比较字符格。
//!
//! 六个状态（纯文本输入 / 带初值与光标 / 枚举 / 数组增删 / 校验失败 /
//! 粘贴换行替换）× 80×24 与 120×40。另三条：反例必须 FAIL、`--no-mods`
//! 回落内置、模块提交的 `_target` 真的到达插件动作。
//!
//! 颜色不是对拍对象：nsh TUI 把前景归到终端 16 色/Reset，内置实现写主题
//! RGB，逐格比色永远不相等（仓库既有 `scripts/mods-parity.sh` 同样只比字符格）。

use std::path::{Path, PathBuf};

mod support;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use hc_tui::app::{ActionForm, App, Prompt, PromptKind};
use hc_tui::config::Config;
use hc_tui::plugin::registry::{ActionInfo, EnablePolicy, Registry};
use hc_tui::ui;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::Terminal;
use serde_json::{json, Value};

const DIALOGS_TSX: &str = include_str!("../examples/mods/dialogs/App.tsx");
const DIALOGS_MANIFEST: &str = include_str!("../examples/mods/dialogs/hctui.toml");
const DIALOGS_NSH: &str = include_str!("../examples/mods/dialogs/nsh.json");
const DIALOGS_CAPABILITIES: &str = r#"{"host":["run","closeOverlay","theme"]}"#;

/// 反例：故意不画校验错误那一行，对拍必须报出差异。
const DROPPED_ERROR_OLD: &str = "errorRow() == index && error()";
const DROPPED_ERROR_NEW: &str = "false";

fn unique_dir(tag: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let unique = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!("hctui-m9f-{tag}-{}-{unique}", std::process::id()))
}

fn config(root: &Path, no_mods: bool) -> Config {
    Config {
        restore: false,
        prompt_new_tab: false,
        no_mods,
        session: Some(format!("hcp-m9f-test-{}", std::process::id())),
        config_path: root.join("config").join("config.toml"),
        state_path: root.join("state").join("state.json"),
        theme: hc_tui::theme::builtin("catppuccin").expect("theme"),
        ..Config::default()
    }
}

fn write_module(config: &Config, tsx: &str) {
    let staging = unique_dir("stage-dialogs");
    std::fs::create_dir_all(staging.join("dist")).expect("staging dist");
    std::fs::write(staging.join("hctui.toml"), DIALOGS_MANIFEST).expect("manifest");
    std::fs::write(staging.join("nsh.json"), DIALOGS_NSH).expect("nsh.json");
    std::fs::write(
        staging.join("dist").join("capabilities.json"),
        DIALOGS_CAPABILITIES,
    )
    .expect("capabilities");
    let code = nsh_bundler::compile_module("App.tsx", tsx, &[]).expect("module compiles");
    std::fs::write(staging.join("dist").join("bundle.js"), code).expect("bundle");
    hc_tui::mods::store::install_local(config, &staging).expect("module installs");
    std::fs::remove_dir_all(staging).ok();
}

fn install_dialogs(config: &Config) {
    if std::env::var_os("M9F_NEGATIVE").is_some() {
        let broken = DIALOGS_TSX.replace(DROPPED_ERROR_OLD, DROPPED_ERROR_NEW);
        assert_ne!(broken, DIALOGS_TSX, "negative control marker is missing");
        write_module(config, &broken);
        return;
    }
    write_module(config, DIALOGS_TSX);
}

fn schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["aa_a"],
        "properties": {
            "aa_a": {"type": "string"},
            "aa_hosts": {"type": "array", "items": {"type": "string"}, "default": ["a", "b"]},
            "dry": {"type": "boolean", "default": true},
            "env": {"type": "string", "enum": ["dev", "prod"], "default": "dev"},
            "name": {"type": "string"}
        }
    })
}

fn action() -> ActionInfo {
    ActionInfo {
        name: "dev.example.form.run".into(),
        package: "dev.example.form".into(),
        action: "run".into(),
        title: "Form".into(),
        description: String::new(),
        contexts: vec!["global".into()],
        args: Some(schema()),
        root: PathBuf::from("/tmp"),
        generation: 1,
        timeout: std::time::Duration::from_secs(5),
        argv: vec!["true".into()],
    }
}

/// 对拍状态：每个状态给出两侧相同的输入序列。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    PromptPlain,
    PromptInitial,
    FormEnum,
    FormArray,
    FormInvalid,
    FormPaste,
}

impl State {
    fn name(&self) -> &'static str {
        match self {
            Self::PromptPlain => "prompt-plain",
            Self::PromptInitial => "prompt-initial",
            Self::FormEnum => "form-enum",
            Self::FormArray => "form-array",
            Self::FormInvalid => "form-invalid",
            Self::FormPaste => "form-paste",
        }
    }
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

fn builtin_open(app: &mut App, state: State) {
    match state {
        State::PromptPlain | State::PromptInitial => {
            let (value, cursor) = if state == State::PromptInitial {
                ("logs", 2)
            } else {
                ("", 0)
            };
            app.prompt = Some(Prompt {
                label: "pane name".into(),
                buffer: value.into(),
                cursor,
                history_pos: None,
                kind: PromptKind::RenamePane(1),
            });
        }
        _ => {
            let mut form =
                ActionForm::build(&action().name, &action().title, &schema(), Value::Null);
            if state == State::FormInvalid {
                // 校验失败后的渲染状态：内置表单的字段行下标 0 是 `aa_a`。
                form.error = Some("args.aa_a is required".into());
                form.error_row = Some(0);
                form.selected = 0;
            }
            app.form = Some(form);
        }
    }
}

fn builtin_input(app: &mut App, state: State) {
    match state {
        State::PromptPlain => {
            app.on_key(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE));
            app.on_key(KeyEvent::new(KeyCode::Char('i'), KeyModifiers::NONE));
        }
        State::PromptInitial => {}
        State::FormEnum => {}
        State::FormArray => {
            // 初始焦点是 `aa_a`；下一行就是 `aa_hosts[0]`。
            app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
            app.on_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
            app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE));
        }
        State::FormInvalid => {}
        State::FormPaste => {
            app.on_paste("a\nb".into());
        }
    }
}

fn module_open(app: &mut App, state: State) -> bool {
    match state {
        State::PromptPlain => app.open_dialogs_prompt(&PromptKind::RenamePane(1), "pane name", ""),
        State::PromptInitial => {
            app.open_dialogs_prompt_at(&PromptKind::RenamePane(1), "pane name", "logs", 2)
        }
        _ => app.open_dialogs_form(&action(), Value::Null),
    }
}

/// 投递一笔输入后画若干帧：模块的重绘、宿主的 resize 应答与 onLayout 都在这
/// 几帧里落地，不靠「内容变了」推断。
fn settle(app: &mut App, terminal: &mut Terminal<TestBackend>) {
    for _ in 0..40 {
        let _ = terminal.clear();
        terminal.draw(|frame| ui::draw(frame, app)).expect("draw");
        app.pump_mods();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

fn key(app: &mut App, terminal: &mut Terminal<TestBackend>, _area: Rect, code: KeyCode) {
    app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
    settle(app, terminal);
}

fn module_input(app: &mut App, terminal: &mut Terminal<TestBackend>, area: Rect, state: State) {
    match state {
        State::PromptPlain => {
            key(app, terminal, area, KeyCode::Char('h'));
            key(app, terminal, area, KeyCode::Char('i'));
        }
        State::PromptInitial => {}
        State::FormEnum => {}
        State::FormArray => {
            key(app, terminal, area, KeyCode::Down);
            app.on_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL));
            settle(app, terminal);
            key(app, terminal, area, KeyCode::Char('c'));
        }
        State::FormInvalid => {
            app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
            settle(app, terminal);
        }
        State::FormPaste => {
            app.on_paste("a\nb".into());
            settle(app, terminal);
        }
    }
}

fn builtin_frame(root: &Path, width: u16, height: u16, state: State) -> (Buffer, Rect) {
    let mut app = App::new(config(root, true)).expect("app builds");
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal
        .draw(|frame| ui::draw(frame, &mut app))
        .expect("draw");
    builtin_open(&mut app, state);
    builtin_input(&mut app, state);
    terminal
        .draw(|frame| ui::draw(frame, &mut app))
        .expect("draw");
    let area = app.content;
    (terminal.backend().buffer().clone(), area)
}

fn module_frame(
    root: &Path,
    width: u16,
    height: u16,
    state: State,
    tsx: Option<&str>,
) -> (Buffer, Rect) {
    let config = config(root, false);
    match tsx {
        Some(tsx) => write_module(&config, tsx),
        None => install_dialogs(&config),
    }
    let mut app = App::new(config).expect("app builds");
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal
        .draw(|frame| ui::draw(frame, &mut app))
        .expect("draw");
    assert!(
        module_open(&mut app, state),
        "module did not take {}",
        state.name()
    );
    support::pump_until_ready(&mut app, &mut terminal, &["dialogs"]);
    let area = app.content;
    module_input(&mut app, &mut terminal, area, state);
    terminal
        .draw(|frame| ui::draw(frame, &mut app))
        .expect("draw");
    app.pump_mods();
    (terminal.backend().buffer().clone(), area)
}

fn compare(builtin: &Buffer, module: &Buffer, area: Rect) -> (usize, Option<String>) {
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
    (differences, first)
}

#[test]
fn dialogs_module_matches_builtin_at_six_states() {
    hc_tui::i18n::set_zh(false);
    let root = unique_dir("parity");
    std::fs::create_dir_all(&root).expect("root");
    let mut failures = Vec::new();
    for (width, height) in [(80u16, 24u16), (120, 40)] {
        for state in [
            State::PromptPlain,
            State::PromptInitial,
            State::FormEnum,
            State::FormArray,
            State::FormInvalid,
            State::FormPaste,
        ] {
            let (builtin, built_area) = builtin_frame(&root, width, height, state);
            let (module, module_area) = module_frame(&root, width, height, state, None);
            assert_eq!(built_area, module_area, "content rect differs");
            let (differences, first) = compare(&builtin, &module, built_area);
            if differences > 0 {
                failures.push(format!(
                    "{} {}x{}: {} differing cells, first {}\nbuiltin:\n{}\nmodule:\n{}",
                    state.name(),
                    width,
                    height,
                    differences,
                    first.unwrap_or_default(),
                    region_text(&builtin, built_area),
                    region_text(&module, module_area),
                ));
            }
        }
    }
    std::fs::remove_dir_all(&root).ok();
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn a_missing_error_line_is_caught() {
    hc_tui::i18n::set_zh(false);
    let root = unique_dir("negative");
    std::fs::create_dir_all(&root).expect("root");
    let broken = DIALOGS_TSX.replace(DROPPED_ERROR_OLD, DROPPED_ERROR_NEW);
    assert_ne!(broken, DIALOGS_TSX, "negative control marker is missing");
    let (builtin, built_area) = builtin_frame(&root, 80, 24, State::FormInvalid);
    let (module, _module_area) = module_frame(&root, 80, 24, State::FormInvalid, Some(&broken));
    let (differences, first) = compare(&builtin, &module, built_area);
    std::fs::remove_dir_all(&root).ok();
    assert!(
        differences > 0,
        "dropping the error line must differ; got {differences} at {:?}",
        first
    );
}

#[test]
fn no_mods_keeps_the_builtin_prompt() {
    hc_tui::i18n::set_zh(false);
    let root = unique_dir("nomods");
    std::fs::create_dir_all(&root).expect("root");
    let mut app = App::new(config(&root, true)).expect("app builds");
    assert!(!app.dialogs_module_available(), "mods are off");
    assert!(!app.open_dialogs_form(&action(), Value::Null));
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("terminal");
    terminal
        .draw(|frame| ui::draw(frame, &mut app))
        .expect("draw");
    app.execute_action("tab.rename", json!({}))
        .expect("opens the builtin prompt");
    assert!(app.prompt.is_some(), "the builtin prompt is still used");
    assert!(app.dialogs.is_none(), "no module dialog is opened");
    terminal
        .draw(|frame| ui::draw(frame, &mut app))
        .expect("draw");
    let text = region_text(terminal.backend().buffer(), app.content);
    assert!(text.contains("rename tab"), "{text}");
    assert!(text.contains("tab name"), "{text}");
    // 参数表单同样回落到 Rust 实现。
    app.prompt = None;
    app.form = Some(ActionForm::build(
        &action().name,
        &action().title,
        &schema(),
        Value::Null,
    ));
    terminal
        .draw(|frame| ui::draw(frame, &mut app))
        .expect("draw");
    let text = region_text(terminal.backend().buffer(), app.content);
    std::fs::remove_dir_all(&root).ok();
    assert!(text.contains("↵ submit"), "{text}");
    assert!(text.contains("aa_hosts"), "{text}");
}

#[test]
fn module_submission_passes_the_target_to_the_action() {
    let root = unique_dir("target");
    std::fs::create_dir_all(&root).expect("root");
    let config = config(&root, false);
    let source = root.join("source").join("dev.example.probe");
    std::fs::create_dir_all(&source).expect("source");
    std::fs::write(
        source.join("plugin.toml"),
        r#"
manifest_version = 1
id = "dev.example.probe"
name = "Probe"
version = "0.1.0"
host_api = 1

[[actions]]
id = "run"
title = "Run"
contexts = ["global", "pane"]
argv = ["sh", "-c", "cat"]
timeout_ms = 20000

[actions.args]
type = "object"
additionalProperties = false
required = ["name"]

[actions.args.properties.name]
type = "string"
"#,
    )
    .expect("manifest");
    hc_tui::plugin::registry::install(&config, &source, EnablePolicy::Enable).expect("installed");
    install_dialogs(&config);
    let mut app = App::new(config).expect("app builds");
    app.plugins = Registry::load(&app.config);
    app.execute_action("space.new", json!({}))
        .expect("workspace created");
    let remote = app.workspaces.len() - 1;
    app.workspaces[remote].machine = Some("peer".into());
    app.workspaces[remote].folder = Some("/srv/remote".into());
    let pane = app.workspaces[remote].tabs[0].panes()[0].0;
    app.panes.get_mut(&pane).expect("pane").terminal = Some("peer/t7".into());
    app.active_ws = 0;
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("terminal");
    terminal
        .draw(|frame| ui::draw(frame, &mut app))
        .expect("draw");
    let probe = ActionInfo {
        name: "dev.example.probe.run".into(),
        package: "dev.example.probe".into(),
        action: "run".into(),
        title: "Run".into(),
        description: String::new(),
        contexts: vec!["global".into(), "pane".into()],
        args: Some(json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["name"],
            "properties": {"name": {"type": "string"}}
        })),
        root: source.clone(),
        generation: 1,
        timeout: std::time::Duration::from_secs(20),
        argv: vec!["sh".into(), "-c".into(), "cat".into()],
    };
    assert!(app.open_dialogs_form(&probe, Value::Null), "form opens");
    app.execute_action(
        "dialogs.submit",
        json!({
            "values": {"name": "ok"},
            "_target": {"pane_or_terminal": "peer/t7"}
        }),
    )
    .expect("submit accepted");
    let request = app.plugin_request.clone().expect("request");
    let view = wait_for_status(&app, &request);
    let context: Value = serde_json::from_str(view.stdout.trim()).expect("context json");
    println!("target context: {context}");
    std::fs::remove_dir_all(&root).ok();
    assert_eq!(context["context"]["terminal"], "peer/t7", "{context}");
    assert_eq!(context["context"]["pane"], pane, "{context}");
    assert_eq!(context["machine"]["workspace"], "peer", "{context}");
}

#[test]
fn prompt_submission_applies_the_rename_and_cancel_clears_it() {
    hc_tui::i18n::set_zh(false);
    let root = unique_dir("submit");
    std::fs::create_dir_all(&root).expect("root");
    let config = config(&root, false);
    install_dialogs(&config);
    let mut app = App::new(config).expect("app builds");
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("terminal");
    terminal
        .draw(|frame| ui::draw(frame, &mut app))
        .expect("draw");
    app.execute_action("tab.rename", json!({}))
        .expect("module opens the prompt");
    assert!(app.dialogs.is_some(), "the module takes the prompt");
    assert!(app.prompt.is_none(), "the builtin prompt stays closed");
    app.execute_action("dialogs.submit", json!({"value": "renamed"}))
        .expect("submit applies");
    assert_eq!(app.workspaces[0].tabs[0].label, "renamed");
    assert!(app.dialogs.is_none(), "submitted dialog is gone");
    app.execute_action("tab.rename", json!({})).expect("reopen");
    app.execute_action("dialogs.cancel", json!({}))
        .expect("cancel");
    assert!(app.dialogs.is_none(), "cancelled dialog is gone");
    assert_eq!(app.workspaces[0].tabs[0].label, "renamed");
    std::fs::remove_dir_all(&root).ok();
}

fn wait_for_status(app: &App, request: &str) -> hc_tui::plugin::runner::View {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        let view = app.plugin_runner.status(request).expect("request exists");
        if view.status.finished() {
            return view;
        }
        assert!(std::time::Instant::now() < deadline, "{request} stuck");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}
