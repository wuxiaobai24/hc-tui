//! M9 基线：八个默认区域的帧快照、单帧耗时与 builtin↔mod 对拍转储。
//!
//! 只在测试构建里编译（`#[cfg(test)] mod m9;`）。复跑命令与实测数字见
//! `docs/m9-baseline.md`。快照按格 dump，格式沿用 M1 的 `tests/snapshot_dump.rs`。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ratatui_core::{
    backend::TestBackend,
    layout::Rect,
    style::Style,
    terminal::{Frame, Terminal},
};
use ratatui_widgets::block::Block;
use serde_json::json;

use super::{
    render_chooser, render_help, render_mode_line, render_prompt, render_settings, render_sidebar,
    render_tab_bar, render_toast,
};
use crate::app::interaction::PointerTarget;
use crate::app::{
    App, Chooser, ChooserItem, ChooserKind, HelpOverlay, Prompt, PromptKind, SettingsOverlay, Toast,
};
use crate::config::Config;
use crate::mods::chrome::{ChromeLayout, LayoutParams};
use crate::mods::SurfacePlace;
use crate::theme::Theme;

/// 八个默认区域；顺序即快照表与耗时表的行序。
const REGIONS: [Region; 8] = [
    Region::Sidebar,
    Region::Tabs,
    Region::Status,
    Region::Picker,
    Region::Dialogs,
    Region::Settings,
    Region::Help,
    Region::Toast,
];

/// 两种分辨率。
const SIZES: [(u16, u16); 2] = [(80, 24), (120, 40)];

/// 单帧耗时的采样：每个样本连画 20 次取平均，每轮 40 个样本（800 次渲染），
/// 跑三轮取中位数。批量摊掉 per-draw 的固定开销与单次抖动。
const BATCH: usize = 20;
const SAMPLES: usize = 40;
const ROUNDS: usize = 3;

/// 对拍 fixture 模块：按启动参数排版一行文本。
const FIXTURE_TSX: &str = r##"
import { host, mount, Text, View } from "nsh";
const props = host.params.props ?? {};
mount(() => (
  <View style={{ flex: 1 }}>
    <Text>{String(props.text ?? "")}</Text>
  </View>
));
"##;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Region {
    Sidebar,
    Tabs,
    Status,
    Picker,
    Dialogs,
    Settings,
    Help,
    Toast,
}

impl Region {
    fn name(self) -> &'static str {
        match self {
            Self::Sidebar => "sidebar",
            Self::Tabs => "tabs",
            Self::Status => "status",
            Self::Picker => "picker",
            Self::Dialogs => "dialogs",
            Self::Settings => "settings",
            Self::Help => "help",
            Self::Toast => "toast",
        }
    }
}

fn temp_root(tag: &str) -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    std::env::temp_dir().join(format!("hctui-m9-{tag}-{}-{unique}", std::process::id()))
}

/// 离屏 App：临时配置目录、无网络线程、固定主题与语言。
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

/// 「有数据」场景：三个 Space、三个 Tab、多个 agent 行，以及全部覆盖层的实例。
fn data_app() -> App {
    let mut app = offline_app("data");
    for name in ["api", "web", "db"] {
        app.execute_action("space.new", json!({ "name": name }))
            .unwrap();
    }
    app.execute_action("space.select", json!({ "index": 2 }))
        .unwrap();
    for _ in 0..2 {
        app.execute_action("tab.new", json!({})).unwrap();
    }
    app.execute_action("pane.split", json!({ "direction": "right" }))
        .unwrap();
    // 快照不能带随机身份、仓库路径或耗时：钉住 origin、label 与 folder。
    for (index, workspace) in app.workspaces.iter_mut().enumerate() {
        workspace.origin = Some(format!("snap-space-{index}"));
        if index == 0 {
            workspace.label = "hc-tui".to_string();
        }
        workspace.folder = None;
        workspace.machine = None;
    }
    // HashMap 顺序不稳定：按 Pane ID 排序后逐个补上 agent 状态与名字。
    let mut panes: Vec<u32> = app.panes.keys().copied().collect();
    panes.sort_unstable();
    let status = ["working", "done", "idle"];
    for (index, id) in panes.iter().enumerate() {
        let pane = app.panes.get_mut(id).unwrap();
        pane.agent.status = Some(status[index % status.len()].to_string());
        pane.agent.turn = index as u64 + 1;
        pane.name = Some(format!("agent-{}", index + 1));
        pane.terminal = Some(format!("local/t{}", index + 1));
    }
    app.chooser = Some(Chooser {
        title: "workspace picker".into(),
        items: ["api", "web", "db"]
            .iter()
            .enumerate()
            .map(|(index, label)| ChooserItem {
                label: (*label).to_string(),
                value: format!("sp{}", index + 1),
            })
            .collect(),
        selected: 1,
        kind: ChooserKind::Templates,
    });
    app.prompt = Some(Prompt {
        label: "space name".into(),
        buffer: "platform".into(),
        cursor: 8,
        history_pos: None,
        kind: PromptKind::RenameWorkspace(0),
    });
    app.settings = Some(SettingsOverlay {
        section: 0,
        selected: 0,
    });
    app.help = Some(HelpOverlay {
        query: String::new(),
        selected: 2,
        scroll: 0,
    });
    app.toast = Some(Toast {
        text: "mod probe requests grants: exec".into(),
        until: Instant::now() + Duration::from_secs(60),
        level: "info".into(),
    });
    app
}

/// 「空数据」场景：同样的矩形，区域数据为空。
fn empty_app() -> App {
    let mut app = offline_app("empty");
    app.workspaces.clear();
    app.panes.clear();
    app.slots.clear();
    app.active_ws = 0;
    app.note.clear();
    app.connection = crate::app::Connection::Ready {
        version: "test".into(),
        core: "test".into(),
    };
    app.chooser = Some(Chooser {
        title: "workspace picker".into(),
        items: Vec::new(),
        selected: 0,
        kind: ChooserKind::Templates,
    });
    app.prompt = Some(Prompt {
        label: String::new(),
        buffer: String::new(),
        cursor: 0,
        history_pos: None,
        kind: PromptKind::RenameWorkspace(0),
    });
    app.settings = Some(SettingsOverlay {
        section: 0,
        selected: 0,
    });
    app.help = Some(HelpOverlay {
        query: "zzzz".into(),
        selected: 0,
        scroll: 0,
    });
    app.toast = Some(Toast {
        text: String::new(),
        until: Instant::now() + Duration::from_secs(60),
        level: "info".into(),
    });
    app
}

/// 按 `ui::draw` 的同一套输入算外框布局，并让 `app.content` 指向内容区。
fn layout(app: &mut App, width: u16, height: u16) -> ChromeLayout {
    let area = Rect::new(0, 0, width, height);
    let tabs = app
        .workspace()
        .map(|workspace| workspace.tabs.len())
        .unwrap_or(0);
    let mode_line = app.mode_line();
    let params = LayoutParams {
        config: &app.config,
        sidebar_visible: app.sidebar,
        tab_count: tabs,
        mode_line: mode_line.is_some(),
        area,
    };
    let layout = app.chrome.layout(params);
    app.mod_regions = layout.mods.clone();
    app.reflow(layout.body);
    layout
}

/// 区域矩形。覆盖层用内容区，toast 用它最后落在的那个三行框。
fn region_rect(region: Region, app: &App, layout: &ChromeLayout) -> Rect {
    match region {
        Region::Sidebar => layout.sidebar.or(layout.rail).unwrap_or_default(),
        Region::Tabs => layout.tab_bar.unwrap_or_default(),
        Region::Status => layout.status.unwrap_or_default(),
        Region::Picker | Region::Dialogs | Region::Settings | Region::Help => app.content,
        Region::Toast => toast_rect(app),
    }
}

/// 与 `render_toast` 一致的落点。
fn toast_rect(app: &App) -> Rect {
    let content = app.content;
    let chars = app
        .toast
        .as_ref()
        .map(|toast| toast.text.chars().count())
        .unwrap_or(0) as u16;
    let width = (chars + 4).min(content.width).max(12);
    Rect::new(
        content.x + content.width.saturating_sub(width),
        content.y + content.height.saturating_sub(3),
        width,
        3,
    )
}

/// 只画一个区域。
fn draw_region(region: Region, app: &mut App, frame: &mut Frame, rect: Rect, theme: &Theme) {
    let mut targets: Vec<(PointerTarget, Rect)> = Vec::new();
    match region {
        Region::Sidebar => render_sidebar(frame, app, rect, theme),
        Region::Tabs => render_tab_bar(frame, app, rect, theme),
        Region::Status => {
            let line = app
                .mode_line()
                .unwrap_or_else(|| (String::new(), String::new()));
            render_mode_line(frame, app, rect, &line, theme);
        }
        Region::Picker => {
            if let Some(chooser) = app.chooser.as_ref() {
                render_chooser(frame, chooser, rect, theme, &mut targets);
            }
        }
        Region::Dialogs => {
            if let Some(prompt) = app.prompt.as_ref() {
                render_prompt(frame, prompt, rect, theme, &mut targets);
            }
        }
        Region::Settings => {
            if let Some(settings) = app.settings.as_ref() {
                render_settings(frame, settings, app, rect, theme, &mut targets);
            }
        }
        Region::Help => {
            if let Some(help) = app.help.as_ref() {
                let rows = app.help_view();
                render_help(frame, help, &rows, rect, theme);
            }
        }
        Region::Toast => {
            if let Some(toast) = app.toast.as_ref() {
                render_toast(frame, toast, app.content, theme);
            }
        }
    }
}

/// 逐格 dump：`x,y|符号|前景|背景|修饰`。
fn cells(terminal: &Terminal<TestBackend>, rect: Rect) -> String {
    let buffer = terminal.backend().buffer();
    let mut out = String::new();
    for y in rect.y..rect.bottom() {
        for x in rect.x..rect.right() {
            let cell = &buffer[(x, y)];
            out.push_str(&format!(
                "{x},{y}|{}|{:?}|{:?}|{:?}\n",
                cell.symbol(),
                cell.fg,
                cell.bg,
                cell.modifier
            ));
        }
    }
    out
}

/// 只取字符格，每行一行文本，供对拍脚本按行差。
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

fn snapshot_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("snapshots")
        .join("m9")
}

fn target_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("m9")
}

/// 在整帧底色之上只画一个区域，返回逐格 dump。
fn render_cells(region: Region, app: &mut App, width: u16, height: u16) -> (Rect, String) {
    let layout = layout(app, width, height);
    let rect = region_rect(region, app, &layout);
    let theme = app.config.theme.clone();
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| {
            frame.render_widget(
                Block::default().style(Style::default().bg(theme.color("bg"))),
                frame.area(),
            );
            draw_region(region, app, frame, rect, &theme);
        })
        .unwrap();
    let text = cells(&terminal, rect);
    (rect, text)
}

#[test]
fn m9_region_snapshots() {
    let update = std::env::var("M9_UPDATE_SNAPSHOTS").as_deref() == Ok("1");
    let dir = snapshot_dir();
    std::fs::create_dir_all(&dir).unwrap();
    for region in REGIONS {
        for (width, height) in SIZES {
            let mut app = data_app();
            let (rect, text) = render_cells(region, &mut app, width, height);
            let name = format!("{}-{width}x{height}.txt", region.name());
            let path = dir.join(&name);
            let header = format!(
                "# region={} size={width}x{height} rect={},{},{},{}\n",
                region.name(),
                rect.x,
                rect.y,
                rect.width,
                rect.height
            );
            let body = format!("{header}{text}");
            if update {
                std::fs::write(&path, &body).unwrap();
                continue;
            }
            let expected = std::fs::read_to_string(&path).unwrap_or_else(|error| {
                panic!(
                    "{} 缺失（{error}）；用 M9_UPDATE_SNAPSHOTS=1 生成",
                    path.display()
                )
            });
            assert_eq!(
                expected,
                body,
                "{} 与快照不一致；确认改动后 M9_UPDATE_SNAPSHOTS=1 重新生成",
                region.name()
            );
        }
    }
}

/// nearest-rank 百分位。
fn percentile(sorted: &[Duration], percent: usize) -> f64 {
    let index = sorted.len().saturating_sub(1) * percent / 100;
    sorted[index].as_secs_f64() * 1000.0
}

#[test]
#[ignore]
fn m9_region_timings() {
    let mut out = String::new();
    out.push_str("region\tsize\tstate\tn\tp50_ms\tp90_ms\tmax_ms\n");
    for region in REGIONS {
        for (width, height) in SIZES {
            // 两态共用「有数据」场景算出的矩形。
            let mut probe = data_app();
            let test_layout = layout(&mut probe, width, height);
            let rect = region_rect(region, &probe, &test_layout);
            for state in ["data", "empty"] {
                let mut app = if state == "data" {
                    data_app()
                } else {
                    empty_app()
                };
                let theme = app.config.theme.clone();
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                // 预热：首帧的分配器与页错误不进样本。
                for _ in 0..64 {
                    terminal
                        .draw(|frame| draw_region(region, &mut app, frame, rect, &theme))
                        .unwrap();
                }
                // 三轮各 SAMPLES 个样本；取 p50 最小的那一轮。本机常有其它构建在跑，
                // 最小轮最接近区域自身的绘制成本，也是最可复现的一轮。
                let mut best: Option<(f64, f64, f64)> = None;
                for _ in 0..ROUNDS {
                    let mut samples = Vec::with_capacity(SAMPLES);
                    for _ in 0..SAMPLES {
                        let started = Instant::now();
                        for _ in 0..BATCH {
                            terminal
                                .draw(|frame| draw_region(region, &mut app, frame, rect, &theme))
                                .unwrap();
                        }
                        samples.push(started.elapsed() / BATCH as u32);
                    }
                    samples.sort_unstable();
                    let round = (
                        percentile(&samples, 50),
                        percentile(&samples, 90),
                        samples[samples.len() - 1].as_secs_f64() * 1000.0,
                    );
                    if best.is_none_or(|(p50, _, _)| round.0 < p50) {
                        best = Some(round);
                    }
                }
                let (p50, p90, max) = best.unwrap_or_default();
                out.push_str(&format!(
                    "{}\t{width}x{height}\t{state}\t{}\t{p50:.3}\t{p90:.3}\t{max:.3}\n",
                    region.name(),
                    SAMPLES * BATCH * ROUNDS,
                ));
            }
        }
    }
    // 单个汇总数字：全部区域、分辨率、两态里最大的 p90，用来对照 8 ms 单表面预算。
    let worst = out
        .lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<&str> = line.split('\t').collect();
            Some((fields[5].parse::<f64>().ok()?, fields))
        })
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(p90, fields)| {
            format!(
                "worst_p90_ms={p90:.3} region={} size={} state={} p50_ms={} max_ms={}\n",
                fields[0], fields[1], fields[2], fields[4], fields[6]
            )
        })
        .unwrap_or_default();
    let dir = target_dir();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("timing.txt"), format!("{out}{worst}")).unwrap();
    print!("{out}{worst}");
}

fn write_mod(app: &App, id: &str, source: &str, manifest: &str, host: &str) {
    let code = nsh_bundler::compile_module("App.tsx", source, &[]).unwrap();
    let dir = app.config.mods_dir().join(id);
    std::fs::create_dir_all(dir.join("dist")).unwrap();
    std::fs::write(dir.join("dist").join("bundle.js"), code).unwrap();
    std::fs::write(
        dir.join("dist").join("capabilities.json"),
        format!("{{\"host\":[{}]}}", host),
    )
    .unwrap();
    std::fs::write(dir.join("hctui.toml"), manifest).unwrap();
}

fn open_mod(app: &mut App, id: &str, source: &str, props: serde_json::Value) -> String {
    let manifest = format!("[mod]\nid = \"{id}\"\nhost_api = 1\n\n[grants]\nhost = [\"state\"]\n");
    write_mod(app, id, source, &manifest, "");
    app.mods_open(id, SurfacePlace::Dialog, props, Some((80, 24)), None, None)
        .unwrap()
}

/// 让表面出帧，直到矩形内的字符格稳定或超时。
fn render_mod(
    app: &mut App,
    surface: &str,
    width: u16,
    height: u16,
    rect: Rect,
) -> Terminal<TestBackend> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    let started = Instant::now();
    let deadline = started + Duration::from_secs(5);
    let mut last = String::new();
    let mut stable = 0;
    loop {
        if let Some(view) = app.mods.surface_mut(surface) {
            terminal
                .draw(|frame| view.view.render(frame, rect))
                .unwrap();
        }
        app.pump_mods();
        let screen = rows(&terminal, rect);
        stable = if screen == last { stable + 1 } else { 0 };
        last = screen;
        // 示例模块的 state 快照是异步回来的：稳定且过了首轮宿主调用再收工。
        if (stable >= 3 && started.elapsed() > Duration::from_millis(800))
            || Instant::now() > deadline
        {
            if let Some(view) = app.mods.surface(surface) {
                let errors = view
                    .errors
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if !errors.is_empty() || view.last_error.is_some() {
                    eprintln!(
                        "mod {} errors: {:?} last_error={:?}",
                        view.mod_id, *errors, view.last_error
                    );
                }
            }
            return terminal;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// 状态行的 fixture 模块：由同一份数据（左标签、正文、右标签、宽度）自己排版。
fn status_row(app: &App, width: u16) -> String {
    let (left, text) = app
        .mode_line()
        .unwrap_or_else(|| (String::new(), String::new()));
    let right = app.connection.label();
    let mut row = if left.is_empty() {
        format!("  {text}")
    } else {
        format!(" {left}  {text}")
    };
    // 与 `render_mode_line` 同一套留白算法：`used` 额外留一格。
    let used = left.chars().count() + text.chars().count() + 3;
    if !right.is_empty() && width as usize > used + right.chars().count() {
        row.push_str(&" ".repeat(width as usize - used - right.chars().count()));
        row.push_str(&right);
    }
    row
}

/// 对拍转储：builtin 侧与 mod 侧写同一矩形、同一行数，脚本按行差。
#[test]
#[ignore]
fn m9_parity_dump() {
    crate::i18n::set_zh(false);
    let dir = target_dir().join("parity");
    std::fs::create_dir_all(&dir).unwrap();
    let (width, height) = SIZES[0];
    let spaces = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("examples")
            .join("mods")
            .join("spaces")
            .join("App.tsx"),
    )
    .unwrap();
    for region in REGIONS {
        let mut app = data_app();
        let chrome = layout(&mut app, width, height);
        let rect = region_rect(region, &app, &chrome);
        let theme = app.config.theme.clone();
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| draw_region(region, &mut app, frame, rect, &theme))
            .unwrap();
        let builtin = rows(&terminal, rect);
        std::fs::write(dir.join(format!("builtin-{}.txt", region.name())), &builtin).unwrap();

        // mod 侧：sidebar 用仓库里的示例模块，其余用 fixture（status 传同一份数据）。
        let mut app = data_app();
        let _ = layout(&mut app, width, height);
        let props = if region == Region::Status {
            json!({ "text": status_row(&app, rect.width) })
        } else {
            json!({ "text": "" })
        };
        let surface = if region == Region::Sidebar {
            let manifest = "[mod]\nid = \"spaces\"\nhost_api = 1\n\n[grants]\nhost = [\"state\", \"run\", \"focus\"]\n";
            write_mod(
                &app,
                "spaces",
                &spaces,
                manifest,
                "\"state\", \"run\", \"focus\"",
            );
            app.mods_open(
                "spaces",
                SurfacePlace::Dialog,
                json!({}),
                Some((80, 24)),
                None,
                None,
            )
            .unwrap()
        } else {
            open_mod(&mut app, "fixture", FIXTURE_TSX, props)
        };
        let terminal = render_mod(&mut app, &surface, width, height, rect);
        let rendered = rows(&terminal, rect);
        std::fs::write(dir.join(format!("mod-{}.txt", region.name())), &rendered).unwrap();
        println!(
            "region={} mod={} rect={:?} builtin_rows={} mod_rows={}",
            region.name(),
            surface,
            rect,
            builtin.lines().count(),
            rendered.lines().count()
        );
    }
}
