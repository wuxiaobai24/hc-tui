//! M9b 逐格对等：同一份状态、同一矩形，builtin 与模块渲染的 Buffer 必须逐格相同。
//!
//! 两套主题来自 `tests/fixtures/m9b_themes.toml`——取值是 nsh TUI 渲染器能逐格
//! 表达的那一档（前景 Reset、对比色块字面 RGB）；默认主题下的颜色量化差异见票据
//! result 的「存疑」段与 `evidence/02-nsh-cell-probe.txt`。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use hc_tui::app::App;
use hc_tui::config::Config;
use hc_tui::model::{Tab, Workspace};
use hc_tui::theme::{self, Theme};
use hc_tui::ui;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::Terminal;

const FIXTURE_THEMES: &str = include_str!("fixtures/m9b_themes.toml");
const SIDEBAR_TSX: &str = include_str!("../examples/mods/sidebar/App.tsx");
const SIDEBAR_MANIFEST: &str = include_str!("../examples/mods/sidebar/hctui.toml");
const SIDEBAR_NSH: &str = include_str!("../examples/mods/sidebar/nsh.json");

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
    // `nsh build` 的产物：运行期按这份能力表判定 host 方法，缺了就是未授予。
    std::fs::write(staging.join("dist").join("capabilities.json"), capabilities)
        .expect("staging capabilities");
    let code = nsh_bundler::compile_module("App.tsx", tsx, &[]).expect("module compiles");
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

fn draw(app: &mut App, width: u16, height: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal.draw(|frame| ui::draw(frame, app)).expect("draw");
    terminal.backend().buffer().clone()
}

/// 模块表面异步出帧：一直画到侧栏矩形连续两帧相同。
fn draw_module_until_stable(app: &mut App, width: u16, height: u16, area: Rect) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    let mut previous = String::new();
    for round in 0..200 {
        terminal.draw(|frame| ui::draw(frame, app)).expect("draw");
        app.tick();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let text = region_text(terminal.backend().buffer(), area);
        if round >= 40 && text == previous && text.contains("│") {
            return terminal.backend().buffer().clone();
        }
        previous = text;
    }
    for surface in &app.mods.surfaces {
        println!(
            "surface {} mod={} place={:?} state={:?} area={:?} error={:?}",
            surface.id,
            surface.mod_id,
            surface.place,
            surface.state,
            surface.area,
            surface.last_error
        );
    }
    println!("note={:?} mod_regions={:?}", app.note, app.mod_regions);
    for surface in &app.mods.surfaces {
        let errors = surface
            .errors
            .lock()
            .map(|errors| errors.clone())
            .unwrap_or_default();
        println!("surface {} errors={errors:?}", surface.id);
    }
    println!(
        "grants={:?} pending={:?}",
        app.mods.grants_for("sidebar"),
        app.mods.pending_grants
    );
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

fn color_name(color: ratatui::style::Color) -> String {
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

fn sidebar_area(_width: u16, height: u16, sidebar_width: u16) -> Rect {
    Rect::new(0, 0, sidebar_width, height)
}

#[test]
fn sidebar_parity_matches_builtin_cell_for_cell() {
    for (width, height) in [(80u16, 24u16), (120, 40)] {
        for theme_index in 0..parity_themes().len() {
            let theme = parity_themes().remove(theme_index);
            let name = theme.name.clone();
            let root = unique_dir(&format!("parity-{width}x{height}-{theme_index}"));
            let config = config_in(&root, theme);

            // builtin：同一份配置与状态，不装模块。
            let mut plain = app_in(config.clone());
            let builtin = draw(&mut plain, width, height);

            // module：装上侧栏模块，外框把它绑到同一矩形。
            install_module(
                &config,
                "sidebar",
                SIDEBAR_MANIFEST,
                SIDEBAR_NSH,
                "{\"host\":[\"state\"]}",
                SIDEBAR_TSX,
            );
            let mut app = app_in(config);
            let area = sidebar_area(width, height, app.config.sidebar_width);
            let module = draw_module_until_stable(&mut app, width, height, area);
            assert!(
                app.mod_regions.iter().any(|region| region.id == "sidebar"),
                "sidebar module must own the region"
            );

            let differences = compare(
                &format!("sidebar {width}x{height} {name}"),
                &builtin,
                &module,
                area,
            );
            println!(
                "sidebar {width}x{height} {name}: {} of {} cells differ",
                differences,
                u32::from(area.width) * u32::from(area.height)
            );
            std::fs::remove_dir_all(root).ok();
            assert_eq!(
                differences, 0,
                "sidebar parity failed at {width}x{height} {name}"
            );
        }
    }
}
