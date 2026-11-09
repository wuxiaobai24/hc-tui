//! M9b 逐格对等：同一份状态、同一矩形，builtin 与模块渲染的 Buffer 必须逐格相同。
//!
//! 两套主题来自 `tests/fixtures/m9b_themes.toml`——取值是 nsh TUI 渲染器能逐格
//! 表达的那一档（前景 Reset、对比色块用饱和 RGB）；默认主题下的颜色量化差异见
//! 票据 result 的「存疑」段与 `evidence/02-nsh-cell-probe.txt`。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

mod support;

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
    // 序号来自原子计数：并发用例不会在同一纳秒拿到同一个目录。
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let unique = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!("hctui-m9b-{tag}-{}-{unique}", std::process::id()))
}

fn config_in(root: &Path, theme: Theme, fixture: Fixture) -> Config {
    Config {
        restore: false,
        prompt_new_tab: false,
        config_path: root.join("config").join("config.toml"),
        state_path: root.join("state").join("state.json"),
        theme,
        tab_bar_position: if fixture.bottom_tabs {
            hc_tui::config::TabBarPosition::Bottom
        } else {
            hc_tui::config::TabBarPosition::Top
        },
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

/// 侧栏的状态格：无 Agent、两条 Agent terminal、Tab 栏在底部、机器组折叠。
#[derive(Clone, Copy)]
struct Fixture {
    agents: bool,
    bottom_tabs: bool,
    folded_group: bool,
}

const PLAIN: Fixture = Fixture {
    agents: false,
    bottom_tabs: false,
    folded_group: false,
};

/// 固定状态：一个本机 Space、两个 Tab，可选两条不带本地 Pane 的 Agent terminal。
fn fixture_state(app: &mut App, fixture: Fixture) {
    app.note.clear();
    app.connection = hc_tui::app::Connection::Connecting;
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
    if fixture.folded_group {
        // 与鼠标点机器组头同源：内置侧栏据此换成 ▸ 并收起该组的 Space 行。
        app.folded_agent_groups.insert("machine:local".to_string());
    }
    if fixture.agents {
        app.terminals = vec![
            terminal("local/t1", "claude", "working", 2),
            terminal("local/t2", "codex", "idle", 3),
        ];
    }
}

fn terminal(
    address: &str,
    name: &str,
    status: &str,
    turn: u64,
) -> hc_tui::hc::proto::TerminalSummary {
    hc_tui::hc::proto::TerminalSummary {
        address: address.to_string(),
        parent: None,
        space_id: Some("space:1".to_string()),
        tab_id: None,
        name: Some(name.to_string()),
        title: name.to_string(),
        cwd: Some("/nonexistent/m9b-fixture".to_string()),
        program: Some(name.to_string()),
        space: Some("work".to_string()),
        plugin: None,
        plugin_terminal: None,
        session: None,
        turn,
        seen_turn: turn,
        pinned: false,
        cols: 80,
        rows: 24,
        live: true,
        status: Some(status.to_string()),
        keep: false,
    }
}

fn app_in(config: Config, fixture: Fixture) -> App {
    let mut app = App::new(config).expect("app builds");
    fixture_state(&mut app, fixture);
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

/// 模块表面异步出帧：画到三块区域都按宿主交付的 props 落笔。
fn draw_module_when_ready(app: &mut App, width: u16, height: u16, modules: &[&str]) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    support::pump_until_ready(app, &mut terminal, modules);
    terminal.backend().buffer().clone()
}

fn region_text(buffer: &Buffer, area: Rect) -> String {
    support::region_text(buffer, area)
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

/// 一套主题下三块区域的差异条数（模块, 差异, 总格数）。断言用与量差距用同一段流程。
fn region_differences(
    width: u16,
    height: u16,
    theme: Theme,
    label: &str,
    fixture: Fixture,
) -> Vec<(&'static str, usize, usize)> {
    let root = unique_dir(&format!("diff-{label}-{width}x{height}"));
    let config = config_in(&root, theme, fixture);
    let mut plain = app_in(config.clone(), fixture);
    let builtin = draw(&mut plain, width, height);
    let (sidebar, tab_bar, status) = builtin_areas(&plain, width, height);
    for module in &REGION_MODS {
        install_module(&config, module, "{\"host\":[\"state\"]}");
    }
    let mut app = app_in(config, fixture);
    let frame = draw_module_when_ready(&mut app, width, height, &["sidebar", "tabs", "status"]);
    let mut out = Vec::new();
    for (module, area) in [
        ("sidebar", sidebar.unwrap()),
        ("tabs", tab_bar.unwrap()),
        ("status", status.unwrap()),
    ] {
        let differences = compare(
            &format!("{module} {width}x{height} {label}"),
            &builtin,
            &frame,
            area,
        );
        out.push((
            module,
            differences,
            usize::from(area.width) * usize::from(area.height),
        ));
    }
    std::fs::remove_dir_all(root).ok();
    out
}

#[test]
fn regions_match_builtin_cell_for_cell() {
    let _core = support::IsolatedCore::new("m9b");
    for (width, height) in [(80u16, 24u16), (120, 40)] {
        for theme in parity_themes() {
            let name = theme.name.clone();
            for (module, differences, total) in
                region_differences(width, height, theme, &name, PLAIN)
            {
                println!("{module} {width}x{height} {name}: {differences} of {total} cells differ");
                assert_eq!(
                    differences, 0,
                    "{module} parity failed at {width}x{height} {name}"
                );
            }
        }
    }
}

/// 侧栏里两条 Agent terminal 的状态：分组头、徽标、标签都要与内置实现逐格相同。
#[test]
fn regions_match_builtin_with_agent_terminals() {
    let _core = support::IsolatedCore::new("m9b");
    let fixture = Fixture {
        agents: true,
        bottom_tabs: false,
        folded_group: false,
    };
    for (width, height) in [(80u16, 24u16), (120, 40)] {
        for theme in parity_themes() {
            let name = theme.name.clone();
            for (module, differences, total) in
                region_differences(width, height, theme, &name, fixture)
            {
                println!(
                    "{module} agents {width}x{height} {name}: {differences} of {total} cells differ"
                );
                assert_eq!(
                    differences, 0,
                    "{module} agents parity at {width}x{height} {name}"
                );
            }
        }
    }
}

/// 外框把 Tab 栏放到底部时三块仍然逐格相同。
#[test]
fn regions_match_builtin_with_bottom_tab_bar() {
    let _core = support::IsolatedCore::new("m9b");
    let fixture = Fixture {
        agents: true,
        bottom_tabs: true,
        folded_group: false,
    };
    for (width, height) in [(80u16, 24u16), (120, 40)] {
        for theme in parity_themes() {
            let name = theme.name.clone();
            for (module, differences, total) in
                region_differences(width, height, theme, &name, fixture)
            {
                println!(
                    "{module} bottom-tabs {width}x{height} {name}: {differences} of {total} cells differ"
                );
                assert_eq!(
                    differences, 0,
                    "{module} bottom-tabs parity at {width}x{height} {name}"
                );
            }
        }
    }
}

/// 机器组折叠：组头换 `▸`、该组的 Space 行收起，与内置实现逐格相同。
#[test]
fn regions_match_builtin_with_folded_machine_group() {
    let _core = support::IsolatedCore::new("m9b");
    let fixture = Fixture {
        agents: true,
        bottom_tabs: false,
        folded_group: true,
    };
    for (width, height) in [(80u16, 24u16), (120, 40)] {
        for theme in parity_themes() {
            let name = theme.name.clone();
            for (module, differences, total) in
                region_differences(width, height, theme, &name, fixture)
            {
                println!(
                    "{module} folded {width}x{height} {name}: {differences} of {total} cells differ"
                );
                assert_eq!(
                    differences, 0,
                    "{module} folded-group parity at {width}x{height} {name}"
                );
            }
        }
    }
}

/// 默认主题下的量化差距：nsh 渲染器把前景色归到 16 色/Reset、把背景色归到
/// bandless 档位，而内置实现写的是主题 RGB。这条用例只量差距，不做断言。
#[test]
#[ignore = "measures the known color quantization gap of the default theme"]
fn default_theme_color_gap_is_measurable() {
    let _core = support::IsolatedCore::new("m9b");
    for (width, height) in [(80u16, 24u16), (120, 40)] {
        let theme = theme::builtin("default").expect("default theme");
        for (module, differences, total) in
            region_differences(width, height, theme, "default", PLAIN)
        {
            println!("{module} {width}x{height} default: {differences} of {total} cells differ");
        }
    }
}

/// 首份 props 延迟 500 ms 才生效的侧栏：在这之前一行都不画。
const DELAYED_PROPS_TSX: &str = r##"
import { host, mount, signal, Text, View } from "nsh";
const params: any = (host as any).params ?? {};
const props = signal<any>({});
const ready = signal(false);
// 首份 props 迟到 500 ms：等到了才落笔。
setTimeout(() => {
  props.set(params.props ?? {});
  ready.set(true);
}, 500);
mount(() => (
  <View style={{ flex: 1 }}>
    {ready() ? <Text>{`late props ${props().indicators}`}</Text> : null}
  </View>
));
"##;

/// 启动期的每一帧只能是两种画面：完整的内置渲染，或完整的模块渲染。
#[test]
fn startup_frames_never_show_a_partial_region() {
    let _core = support::IsolatedCore::new("m9b");
    let (width, height) = (80u16, 24u16);
    let theme = parity_themes().remove(0);
    let root = unique_dir("startup-frames");
    // 内置基线：同一份状态的完整侧栏。
    let mut baseline = app_in(config_in(&root, theme.clone(), PLAIN), PLAIN);
    let builtin_frame = draw(&mut baseline, width, height);
    let sidebar = builtin_areas(&baseline, width, height)
        .0
        .expect("sidebar area");
    let builtin = region_text(&builtin_frame, sidebar);

    // 模块终态：同一份状态、同一个矩形，等它按首份 props 与 state 落笔。
    let config = config_in(&root, theme, PLAIN);
    for module in &REGION_MODS {
        install_module(&config, module, "{\"host\":[\"state\"]}");
    }
    let mut settled = app_in(config.clone(), PLAIN);
    let settled_frame = draw_module_when_ready(&mut settled, width, height, &["sidebar"]);
    let module = region_text(&settled_frame, sidebar);
    let same = builtin == module;

    // 从头再跑一次启动：中途任何一帧都必须是上面两种画面之一。
    let mut app = app_in(config, PLAIN);
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    let (mut frames, mut builtin_frames, mut module_frames) = (0, 0, 0);
    let mut takeover: Option<usize> = None;
    for round in 0..1200 {
        // 这一帧由谁画像素：帧开始时的就绪状态决定（就绪前会盖回内置帧）。
        let authority = if app.region_ready("sidebar") {
            "module"
        } else {
            "builtin"
        };
        terminal
            .draw(|frame| ui::draw(frame, &mut app))
            .expect("draw");
        app.pump_mods();
        let shown = region_text(terminal.backend().buffer(), sidebar);
        if shown != builtin && shown != module {
            panic!("frame {round} is neither the builtin nor the module render:\n{shown}");
        }
        if authority == "module" {
            module_frames += 1;
        } else {
            builtin_frames += 1;
        }
        println!(
            "frame {round:02} authority={authority} complete {:?}",
            shown.lines().nth(2).unwrap_or("")
        );
        frames += 1;
        if takeover.is_none() && app.mod_ready("sidebar") {
            takeover = Some(round);
        }
        // 交接之后再记两帧模块自己的像素，序列能看出切换。
        if takeover.is_some_and(|at| round >= at + 2) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    println!(
        "{frames} frames: builtin {builtin_frames}, module {module_frames}, builtin {} module",
        if same { "==" } else { "!=" }
    );
    assert!(
        builtin_frames > 0,
        "the startup must begin on the builtin render"
    );
    assert!(module_frames > 0, "the module must take the region over");
    std::fs::remove_dir_all(root).ok();
}

/// 首份 props 迟到 500 ms：这段时间区域逐格显示内置内容，不是空白或半份。
#[test]
fn delayed_first_props_keeps_the_builtin_region() {
    let _core = support::IsolatedCore::new("m9b");
    let (width, height) = (80u16, 24u16);
    let theme = parity_themes().remove(0);
    let root = unique_dir("late-props");
    // 基线：同一份状态、同一个矩形的内置帧。
    let mut baseline = app_in(config_in(&root, theme.clone(), PLAIN), PLAIN);
    let builtin = draw(&mut baseline, width, height);
    let sidebar = builtin_areas(&baseline, width, height)
        .0
        .expect("sidebar area");
    let expected = region_text(&builtin, sidebar);

    let config = config_in(&root, theme, PLAIN);
    install_module(
        &config,
        &RegionMod {
            id: "sidebar",
            tsx: DELAYED_PROPS_TSX,
            manifest: SIDEBAR_MANIFEST,
            nsh: SIDEBAR_NSH,
        },
        "{\"host\":[\"state\"]}",
    );
    let mut app = app_in(config, PLAIN);
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    // 首份 props 到达之前每画一帧都必须是内置内容。
    let started = std::time::Instant::now();
    let deadline = std::time::Duration::from_millis(400);
    let mut frames = 0;
    while started.elapsed() < deadline {
        terminal
            .draw(|frame| ui::draw(frame, &mut app))
            .expect("draw");
        app.pump_mods();
        let shown = region_text(terminal.backend().buffer(), sidebar);
        println!(
            "frame {frames:02} builtin={} {:?}",
            shown == expected,
            shown.lines().nth(2).unwrap_or("")
        );
        assert_eq!(
            shown, expected,
            "frame {frames}: region left the builtin render"
        );
        frames += 1;
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(
        frames > 5,
        "the delay window must cover several frames, got {frames}"
    );
    // 500 ms 之后模块落笔，区域换成模块自己的内容。
    support::pump_until_ready(&mut app, &mut terminal, &["sidebar"]);
    for round in 0..3 {
        terminal
            .draw(|frame| ui::draw(frame, &mut app))
            .expect("draw");
        app.pump_mods();
        let shown = region_text(terminal.backend().buffer(), sidebar);
        println!(
            "frame take{round} builtin={} {:?}",
            shown == expected,
            shown.lines().next().unwrap_or("")
        );
    }
    let late = region_text(terminal.backend().buffer(), sidebar);
    assert!(
        late.contains("late props"),
        "the module must take the region over:\n{late}"
    );
    assert_ne!(
        late, expected,
        "the module content must replace the builtin"
    );
    std::fs::remove_dir_all(root).ok();
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
            }
            count += 1;
        }
        runs.push(format!("{count}x{:?}/{:?}/{:?}", last.0, last.1, last.2));
        out.push_str(&format!("sty{y:02} |{}|\n", runs.join(";")));
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
    let _core = support::IsolatedCore::new("m9b");
    for (width, height) in [(80u16, 24u16), (120, 40)] {
        let root = unique_dir(&format!("no-mods-{width}x{height}"));
        let mut config = config_in(&root, parity_themes().remove(0), PLAIN);
        // 模块装好但整体停用：三块必须仍由内置实现绘制。
        for module in &REGION_MODS {
            install_module(&config, module, "{\"host\":[\"state\"]}");
        }
        config.no_mods = true;
        let mut app = app_in(config, PLAIN);
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
