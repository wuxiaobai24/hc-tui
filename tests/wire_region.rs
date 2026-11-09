//! wire 票据自证：清单的 `[module] region` 声明接管内置区域。
//!
//! 三条断言对应票据验收：
//! - 只注册（写清单 + 装包）不改核心，模块立刻接管状态行；停用后回落到内置。
//! - 同一区域被两个包声明时，id 字典序最小的胜出，另一个拿到一条 issue。
//! - 非法区域名在清单校验期被拒绝，错误带 `文件:行:列`。

mod support;

use std::path::{Path, PathBuf};

use hc_tui::app::{App, Connection};
use hc_tui::config::Config;
use hc_tui::model::{Tab, Workspace};
use hc_tui::plugin::registry::Registry;
use hc_tui::ui;
use ratatui::backend::TestBackend;
use ratatui::Terminal;

const PROBE_TSX: &str = r#"
import { mount, Text, View } from "nsh";
function App() {
  return (
    <View style={{ flex: 1, flexDirection: "row" }}>
      <Text>WIRE-PROBE-STATUS</Text>
    </View>
  );
}
mount(App);
"#;

fn unique_dir(tag: &str) -> PathBuf {
    let dir = support::unique_dir("hctui-wire", tag);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn config_in(root: &Path) -> Config {
    Config {
        restore: false,
        prompt_new_tab: false,
        config_path: root.join("config").join("config.toml"),
        state_path: root.join("state").join("state.json"),
        ..Config::default()
    }
}

/// 写清单：`region` 为空表示不声明接管。
fn manifest(id: &str, region: Option<&str>) -> String {
    let region = region
        .map(|region| format!("region = \"{region}\"\n"))
        .unwrap_or_default();
    format!(
        "manifest_version = 2\nid = \"{id}\"\nname = \"{id}\"\nversion = \"0.1.0\"\nhost_api = 1\n\n[module]\nbundle = \"dist/bundle.js\"\nplace = \"none\"\n{region}"
    )
}

/// 把一个包写进开发模块目录；`bundle` 给 `None` 时用当前源码编译一份真的 bundle。
fn write_package(config: &Config, id: &str, region: Option<&str>, bundle: Option<&[u8]>) {
    let root = config.mods_dir().join(id);
    std::fs::create_dir_all(root.join("dist")).expect("package dist");
    std::fs::write(root.join("hctui.toml"), manifest(id, region)).expect("manifest");
    std::fs::write(
        root.join("nsh.json"),
        format!("{{\"name\":\"{id}\",\"id\":\"{id}\",\"version\":\"0.1.0\",\"build\":1}}"),
    )
    .expect("nsh.json");
    let code = match bundle {
        Some(code) => code.to_vec(),
        None => nsh_bundler::compile_module("App.tsx", PROBE_TSX, &[])
            .expect("module compiles")
            .into_bytes(),
    };
    std::fs::write(root.join("dist").join("bundle.js"), code).expect("bundle");
}

fn app_in(config: Config) -> App {
    let mut app = App::new(config).expect("app builds");
    app.connection = Connection::Connecting;
    app.workspaces = vec![Workspace {
        origin: None,
        machine: None,
        folder: Some("/nonexistent/wire-fixture".to_string()),
        id: 1,
        label: "work".to_string(),
        tabs: vec![Tab::new(11, 101)],
        active: 0,
    }];
    app
}

/// 画到状态行稳定为止，返回状态行文本。
fn status_row(app: &mut App, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    let mut previous = String::new();
    for round in 0..300 {
        terminal.draw(|frame| ui::draw(frame, app)).expect("draw");
        app.pump_mods();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let buffer = terminal.backend().buffer();
        let y = buffer.area.bottom() - 1;
        let row: String = (buffer.area.x..buffer.area.right())
            .map(|x| buffer[(x, y)].symbol())
            .collect();
        // 前 40 帧留给宿主应答 `state`：早于它稳定下来的帧只是空壳。
        if round >= 40 && row == previous {
            return row;
        }
        previous = row;
    }
    previous
}

#[test]
fn a_manifest_region_takes_the_status_line_and_disable_restores_the_builtin() {
    let _core = support::IsolatedCore::new("wire");
    let root = unique_dir("status");
    let config = config_in(&root);
    write_package(&config, "wire-probe", Some("status"), None);

    let registry = Registry::load(&config);
    assert_eq!(
        registry.regions.get("status").map(String::as_str),
        Some("wire-probe"),
        "{:?}",
        registry.issues
    );
    assert!(
        registry.issues.is_empty(),
        "no conflicts expected: {:?}",
        registry.issues
    );

    let mut app = app_in(config.clone());
    let row = status_row(&mut app, 80, 24);
    assert!(
        row.contains("WIRE-PROBE-STATUS"),
        "the module must paint the status row, got {row:?}"
    );
    assert!(
        app.mod_regions
            .iter()
            .any(|region| region.id == "wire-probe"),
        "the status area must carry the module block: {:?}",
        app.mod_regions
    );

    // 停用走既有的 250ms 重载路径：注册表指纹变化后重新加载并撤下区域。
    hc_tui::pkg::set_enabled(&config, "wire-probe", false).expect("disable");
    app.refresh_plugins_if_changed();
    assert!(
        app.plugins.regions.is_empty(),
        "a disabled package must release the region: {:?}",
        app.plugins.regions
    );
    let row = status_row(&mut app, 80, 24);
    assert!(
        !row.contains("WIRE-PROBE-STATUS"),
        "the builtin status line must come back, got {row:?}"
    );
    assert!(
        !app.mod_regions
            .iter()
            .any(|region| region.id == "wire-probe"),
        "the module block must be gone: {:?}",
        app.mod_regions
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn two_packages_declaring_one_region_report_the_loser_and_keep_the_smallest_id() {
    let _core = support::IsolatedCore::new("wire");
    let root = unique_dir("conflict");
    let config = config_in(&root);
    let bundle = nsh_bundler::compile_module("App.tsx", PROBE_TSX, &[]).expect("module compiles");
    write_package(&config, "wire-a", Some("spaces"), Some(bundle.as_bytes()));
    write_package(&config, "wire-b", Some("spaces"), Some(bundle.as_bytes()));

    let registry = Registry::load(&config);
    assert_eq!(
        registry.regions.get("spaces").map(String::as_str),
        Some("wire-a"),
        "{:?}",
        registry.issues
    );
    let issues = registry.issues_for("wire-b");
    assert_eq!(
        issues,
        vec!["region spaces is already taken by wire-a; wire-b is not bound".to_string()]
    );
    assert!(registry.issues_for("wire-a").is_empty());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn an_unknown_region_is_rejected_with_file_line_and_column() {
    let _core = support::IsolatedCore::new("wire");
    let text = "manifest_version = 2\nid = \"dev.example.probe\"\nname = \"Probe\"\nversion = \"0.1.0\"\nhost_api = 1\n\n[module]\nbundle = \"dist/bundle.js\"\nregion = \"sidebar\"\n";
    let error = hc_tui::pkg::parse_file(Path::new("/tmp/hctui.toml"), text).unwrap_err();
    assert_eq!(error.line, 9, "{error}");
    assert!(
        error
            .message
            .contains("module region must be spaces, tabs or status"),
        "{}",
        error.message
    );
    assert!(
        error.to_string().starts_with("/tmp/hctui.toml:9:"),
        "{error}"
    );
}
