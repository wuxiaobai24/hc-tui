//! M9b 逐格对等：同一份状态、同一矩形，builtin 与模块渲染的 Buffer 必须逐格相同。
//!
//! 两套主题来自 `tests/fixtures/m9b_themes.toml`——取值是 nsh TUI 渲染器能逐格
//! 表达的那一档（前景 Reset、对比色块用饱和 RGB）；默认主题下的颜色量化差异见
//! 票据 result 的「存疑」段与 `evidence/02-nsh-cell-probe.txt`。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use hc_tui::app::App;
use hc_tui::config::Config;
use hc_tui::model::{Tab, Workspace};
use hc_tui::mods::chrome::LayoutParams;
use hc_tui::theme::{self, Theme};
use hc_tui::ui;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::Terminal;

const FIXTURE_THEMES: &str = include_str!("fixtures/m9b_themes.toml");

const SIDEBAR_TSX: &str = include_str!("../examples/mods/sidebar/App.tsx");
const SIDEBAR_MANIFEST: &str = include_str!("../examples/mods/sidebar/hctui.toml");
const SIDEBAR_NSH: &str = include_str!("../examples/mods/sidebar/nsh.json");

const TABS_TSX: &str = include_str!("../examples/mods/tabs/App.tsx");
const TABS_MANIFEST: &str = include_str!("../examples/mods/tabs/hctui.toml");
const TABS_NSH: &str = include_str!("../examples/mods/tabs/nsh.json");

const STATUS_TSX: &str = include_str!("../examples/mods/status/App.tsx");
const STATUS_MANIFEST: &str = include_str!("../examples/mods/status/hctui.toml");
const STATUS_NSH: &str = include_str!("../examples/mods/status/nsh.json");

/// 一条区域模块：模块 ID、源码、清单、能力表。
struct RegionMod {
    id: &'static str,
    tsx: &'static str,
    manifest: &'static str,
    nsh: &'static str,
}

const REGION_MODS: [RegionMod; 3] = [
    RegionMod {
        id: "sidebar",
        tsx: SIDEBAR_TSX,
        manifest: SIDEBAR_MANIFEST,
        nsh: SIDEBAR_NSH,
    },
    RegionMod {
        id: "tabs",
        tsx: TABS_TSX,
        manifest: TABS_MANIFEST,
        nsh: TABS_NSH,
    },
    RegionMod {
        id: "status",
        tsx: STATUS_TSX,
        manifest: STATUS_MANIFEST,
        nsh: STATUS_NSH,
    },
];

#[derive(serde::Deserialize)]
struct ThemesFile {
    theme: Vec<ThemeEntry>,
}

#[derive(serde::Deserialize)]
struct ThemeEntry {
    name: String,
    tokens: BTreeMap<String, String>,
}

fn parity_themes() -> Vec<Theme> {
    let file: ThemesFile = toml::from_str(FIXTURE_THEMES).expect("fixture themes parse");
    file.theme
        .into_iter()
        .map(|entry| {
            let mut theme = theme::builtin("default").expect("default theme");
            theme.name = entry.name;
            for (token, text) in entry.tokens {
                let color = theme::parse_color(&text).expect("fixture color parses");
                theme.set(&token, color);
            }
            theme
        })
        .collect()
}

fn unique_dir(tag: &str) -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!("hctui-m9b-{tag}-{}-{unique}", std::process::id()))
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

/// 把 TSX 源码编译成包并装进临时配置目录。
fn install_module(config: &Config, module: &RegionMod, capabilities: &str) {
    let staging = unique_dir(&format!("src-{}", module.id));
    std::fs::create_dir_all(staging.join("dist")).expect("staging dist");
    std::fs::write(staging.join("hctui.toml"), module.manifest).expect("staging manifest");
    std::fs::write(staging.join("nsh.json"), module.nsh).expect("staging nsh.json");
    // `nsh build` 的产物：运行期按这份能力表判定 host 方法，缺了就是未授予。
    std::fs::write(staging.join("dist").join("capabilities.json"), capabilities)
        .expect("staging capabilities");
    let code = nsh_bundler::compile_module("App.tsx", module.tsx, &[]).expect("module compiles");
    std::fs::write(staging.join("dist").join("bundle.js"), code).expect("staging bundle");
    hc_tui::mods::store::install_local(config, &staging).expect("module installs");
    std::fs::remove_dir_all(staging).ok();
}

/// 固定状态：一个本机 Space、两个 Tab、没有 Agent Pane。
fn fixture_state(app: &mut App) {
    app.note.clear();
    app.workspaces = vec![Workspace {
        origin: None,
        machine: None,
        folder: Some("/nonexistent/m9b-fixture".to_string()),
        id: 1,
        label: "work".to_string(),
        tabs: vec![Tab::new(11, 101), Tab::new(12, 102)],
        active: 0,
    }];
    app.active_ws = 0;
}

fn app_in(config: Config) -> App {
    let mut app = App::new(config).expect("app builds");
    fixture_state(&mut app);
    app
}

/// 内置布局在这一帧给出的三个区域矩形（侧栏、Tab 栏、模式行）。
fn builtin_areas(app: &App, width: u16, height: u16) -> (Option<Rect>, Option<Rect>, Option<Rect>) {
    let tabs = app
        .workspace()
        .map(|workspace| workspace.tabs.len())
        .unwrap_or(0);
    let mode_line = app.mode_line();
    let layout = app.chrome.layout(LayoutParams {
        config: &app.config,
        sidebar_visible: app.sidebar,
        tab_count: tabs,
        mode_line: mode_line.is_some(),
        area: Rect::new(0, 0, width, height),
    });
    (layout.sidebar, layout.tab_bar, layout.status)
}

fn draw(app: &mut App, width: u16, height: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal.draw(|frame| ui::draw(frame, app)).expect("draw");
    terminal.backend().buffer().clone()
}

/// 模块表面异步出帧：画到给定的几块矩形连续两帧相同为止。
fn draw_module_until_stable(app: &mut App, width: u16, height: u16, areas: &[Rect]) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    let mut previous = String::new();
    for round in 0..300 {
        terminal.draw(|frame| ui::draw(frame, app)).expect("draw");
        app.tick();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let text: String = areas
            .iter()
            .map(|area| region_text(terminal.backend().buffer(), *area))
            .collect();
        // 前 40 帧留给宿主应答 `state`：早于它稳定下来的帧只是空壳。
        if round >= 40 && text == previous && text.contains("│") {
            return terminal.backend().buffer().clone();
        }
        previous = text;
    }
    terminal.backend().buffer().clone()
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

fn color_name(color: Color) -> String {
    format!("{color:?}")
}

/// 逐格比较；返回差异条数，并把第一处差异打在 stdout。
fn compare(label: &str, builtin: &Buffer, module: &Buffer, area: Rect) -> usize {
    let mut differences = 0;
    let mut first: Option<String> = None;
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            let (left, right) = (&builtin[(x, y)], &module[(x, y)]);
            let same = left.symbol() == right.symbol()
                && left.fg == right.fg
                && left.bg == right.bg
                && left.modifier == right.modifier;
            if same {
                continue;
            }
            differences += 1;
            if first.is_none() {
                first = Some(format!(
                    "({x},{y}) builtin: sym={:?} fg={} bg={} mod={:?} · module: sym={:?} fg={} bg={} mod={:?}",
                    left.symbol(),
                    color_name(left.fg),
                    color_name(left.bg),
                    left.modifier,
                    right.symbol(),
                    color_name(right.fg),
                    color_name(right.bg),
                    right.modifier,
                ));
            }
        }
    }
    if let Some(line) = first {
        println!("[{label}] first difference {line}");
        println!("[{label}] builtin:\n{}", region_text(builtin, area));
        println!("[{label}] module:\n{}", region_text(module, area));
    }
    differences
}

#[test]
fn regions_match_builtin_cell_for_cell() {
    for (width, height) in [(80u16, 24u16), (120, 40)] {
        for (theme_index, theme) in parity_themes().into_iter().enumerate() {
            let name = theme.name.clone();
            let root = unique_dir(&format!("parity-{width}x{height}-{theme_index}"));
            let config = config_in(&root, theme);

            // builtin：同一份配置与状态，不装模块。
            let mut plain = app_in(config.clone());
            let builtin = draw(&mut plain, width, height);
            let (sidebar, tab_bar, status) = builtin_areas(&plain, width, height);

            // module：装上区域模块，默认外框把它们绑到同一矩形。
            for module in &REGION_MODS {
                install_module(&config, module, "{\"host\":[\"state\"]}");
            }
            let mut app = app_in(config);
            let mut areas: Vec<Rect> = Vec::new();
            for (module, area) in [("sidebar", sidebar), ("tabs", tab_bar), ("status", status)] {
                let area = area.unwrap_or_else(|| panic!("{module} area missing"));
                areas.push(area);
            }
            let frame = draw_module_until_stable(&mut app, width, height, &areas);
            for (module, area) in [
                ("sidebar", sidebar.unwrap()),
                ("tabs", tab_bar.unwrap()),
                ("status", status.unwrap()),
            ] {
                assert!(
                    app.mod_regions.iter().any(|region| region.id == module),
                    "{module} module must own its region"
                );
                let differences = compare(
                    &format!("{module} {width}x{height} {name}"),
                    &builtin,
                    &frame,
                    area,
                );
                println!(
                    "{module} {width}x{height} {name}: {differences} of {} cells differ",
                    u32::from(area.width) * u32::from(area.height)
                );
                assert_eq!(
                    differences, 0,
                    "{module} parity failed at {width}x{height} {name}"
                );
            }
            std::fs::remove_dir_all(root).ok();
        }
    }
}

/// 回退帧的快照：`--no-mods` 下三块仍由内置实现绘制，帧与迁移前一致。
fn frame_snapshot(buffer: &Buffer) -> String {
    let area = buffer.area;
    let mut out = String::new();
    for y in area.y..area.bottom() {
        let symbols: String = (area.x..area.right())
            .map(|x| buffer[(x, y)].symbol())
            .collect();
        out.push_str(&format!("row{y:02} |{symbols}|\n"));
        let mut runs: Vec<String> = Vec::new();
        let mut run = String::new();
        let mut count = 0;
        let mut last = (
            Color::Reset,
            Color::Reset,
            ratatui::style::Modifier::empty(),
        );
        for x in area.x..area.right() {
            let cell = &buffer[(x, y)];
            let style = (cell.fg, cell.bg, cell.modifier);
            if style != last && count > 0 {
                runs.push(format!("{count}x{:?}/{:?}/{:?}", last.0, last.1, last.2));
                count = 0;
            }
            if count == 0 {
                last = style;
                run.clear();
            }
            count += 1;
        }
        runs.push(format!("{count}x{:?}/{:?}/{:?}", last.0, last.1, last.2));
        out.push_str(&format!("sty{y:02} |{}|\n", runs.join(";")));
        let _ = run;
    }
    out
}

fn snapshot_path(width: u16, height: u16) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("m9b_snapshots")
        .join(format!("no-mods-{width}x{height}.txt"))
}

#[test]
fn no_mods_frame_matches_baseline_snapshot() {
    for (width, height) in [(80u16, 24u16), (120, 40)] {
        let root = unique_dir(&format!("no-mods-{width}x{height}"));
        let mut config = config_in(&root, parity_themes().remove(0));
        // 模块装好但整体停用：三块必须仍由内置实现绘制。
        for module in &REGION_MODS {
            install_module(&config, module, "{\"host\":[\"state\"]}");
        }
        config.no_mods = true;
        let mut app = app_in(config);
        let buffer = draw(&mut app, width, height);
        assert!(
            app.mod_regions.is_empty(),
            "--no-mods must not bind module regions"
        );
        let snapshot = frame_snapshot(&buffer);
        let path = snapshot_path(width, height);
        if std::env::var("M9B_BLESS").is_ok() {
            std::fs::create_dir_all(path.parent().expect("snapshot dir")).expect("snapshot dir");
            std::fs::write(&path, &snapshot).expect("snapshot written");
            println!("blessed {}", path.display());
        }
        let expected = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("{}: {error}; run with M9B_BLESS=1", path.display()));
        std::fs::remove_dir_all(root).ok();
        assert_eq!(
            snapshot, expected,
            "no-mods frame differs at {width}x{height}"
        );
        println!("no-mods {width}x{height}: frame matches tests/m9b_snapshots");
    }
}
