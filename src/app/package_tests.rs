//! M11b：统一包的运行期接线——一个包安装三类入口、单一启用闸门、记录里的授权。

use super::*;
use crate::plugin::registry::{EnablePolicy, Registry};
use crate::store::Store;
use ratatui_core::backend::TestBackend;
use ratatui_core::terminal::Terminal;

const PACKAGE: &str = "dev.hctui.pkg.git-board";

fn unique_dir(tag: &str) -> std::path::PathBuf {
    crate::testutil::unique_dir("hctui-pkg", tag)
}

fn config_in(root: &std::path::Path) -> Config {
    Config {
        restore: false,
        prompt_new_tab: false,
        config_path: root.join("config").join("config.toml"),
        state_path: root.join("state").join("state.json"),
        ..Config::default()
    }
}

fn app(config: Config) -> App {
    let (_, events) = std::sync::mpsc::channel();
    App::with_network(config, None, events).unwrap()
}

fn example_package() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("examples")
        .join("packages")
        .join("git-board")
}

fn install_example(config: &Config) {
    crate::plugin::registry::install(config, &example_package(), EnablePolicy::Enable).unwrap();
}

fn frames(app: &mut App, count: usize) -> String {
    let mut terminal = Terminal::new(TestBackend::new(120, 32)).unwrap();
    for _ in 0..count {
        terminal.draw(|frame| crate::ui::draw(frame, app)).unwrap();
        app.tick();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let buffer = terminal.backend().buffer();
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                + "\n"
        })
        .collect()
}

fn docked_module(app: &App) -> bool {
    app.chrome.mods().iter().any(|(id, _)| id == PACKAGE)
}

fn region_surface(app: &App) -> bool {
    app.mods.surfaces.iter().any(|surface| {
        surface.mod_id == PACKAGE && surface.place == crate::mods::SurfacePlace::Region
    })
}

#[test]
fn one_package_installs_actions_terminals_and_a_module() {
    let root = unique_dir("one");
    let config = config_in(&root);
    install_example(&config);
    let registry = Registry::load(&config);
    let actions = registry.actions();
    assert_eq!(actions.len(), 1, "{actions:?}");
    assert_eq!(actions[0].name, format!("{PACKAGE}.summary"));
    let terminals = registry.terminals();
    assert_eq!(terminals.len(), 1, "{terminals:?}");
    assert_eq!(terminals[0].name, format!("{PACKAGE}.git-ui"));
    assert_eq!(terminals[0].argv, vec!["lazygit".to_string()]);
    assert_eq!(registry.modules().len(), 1);

    let mut app = app(config);
    app.sync_package_modules();
    assert!(docked_module(&app), "chrome: {:?}", app.chrome.mods());
    app.sync_mod_surfaces();
    assert!(
        region_surface(&app),
        "surfaces: {}",
        app.mods.surfaces.len()
    );
    let screen = frames(&mut app, 40);
    assert!(screen.contains("recent commits"), "screen: {screen}");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn disabling_a_package_hides_its_module_and_actions() {
    let root = unique_dir("disable");
    let config = config_in(&root);
    install_example(&config);
    let mut app = app(config);
    app.sync_package_modules();
    app.sync_mod_surfaces();
    assert!(docked_module(&app) && region_surface(&app));

    crate::pkg::set_enabled(&app.config, PACKAGE, false).unwrap();
    app.refresh_plugins_if_changed();
    assert!(app.plugins.actions().is_empty());
    assert!(app.plugins.terminals().is_empty());
    assert!(!docked_module(&app), "chrome: {:?}", app.chrome.mods());
    assert_eq!(app.mods.surfaces.len(), 0);
    let screen = frames(&mut app, 4);
    assert!(!screen.contains("recent commits"), "screen: {screen}");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn enable_restores_the_module_surface_without_restart() {
    let root = unique_dir("enable");
    let config = config_in(&root);
    install_example(&config);
    let mut app = app(config);
    app.sync_package_modules();
    app.sync_mod_surfaces();
    assert!(region_surface(&app));

    crate::pkg::set_enabled(&app.config, PACKAGE, false).unwrap();
    app.refresh_plugins_if_changed();
    assert!(!docked_module(&app) && !region_surface(&app));

    crate::pkg::set_enabled(&app.config, PACKAGE, true).unwrap();
    app.refresh_plugins_if_changed();
    app.sync_mod_surfaces();
    assert!(docked_module(&app), "chrome: {:?}", app.chrome.mods());
    assert!(
        region_surface(&app),
        "surfaces: {}",
        app.mods.surfaces.len()
    );
    assert_eq!(app.plugins.actions().len(), 1);
    let screen = frames(&mut app, 40);
    assert!(screen.contains("recent commits"), "screen: {screen}");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn no_mods_keeps_actions_and_terminals_but_skips_the_module() {
    let root = unique_dir("nomods");
    let mut config = config_in(&root);
    install_example(&config);
    config.no_mods = true;
    let entries = Registry::load(&config).entries(&config);
    assert_eq!(entries.len(), 1);
    assert!(entries[0].capabilities.actions);
    assert!(entries[0].capabilities.terminals);
    assert!(entries[0].capabilities.module);

    let mut app = app(config);
    assert_eq!(app.plugins.actions().len(), 1);
    assert_eq!(app.plugins.terminals().len(), 1);
    app.sync_package_modules();
    app.sync_mod_surfaces();
    assert!(!docked_module(&app));
    assert_eq!(app.mods.surfaces.len(), 0);
    let error = app
        .mods_open(
            PACKAGE,
            crate::mods::SurfacePlace::Dialog,
            json!({}),
            Some((60, 10)),
            None,
            None,
        )
        .unwrap_err();
    assert!(error.contains("--no-mods"), "{error}");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn module_grants_come_from_the_package_record() {
    let root = unique_dir("grants");
    let config = config_in(&root);
    install_example(&config);
    // 代际目录里的清单仍声明 exec；记录只保留 state，运行期以记录为准。
    let store = Store::open(config.plugins_state_dir());
    let mut record = store.record(PACKAGE).unwrap();
    let root_dir = store.generation_dir(PACKAGE, record.generation);
    let manifest = std::fs::read_to_string(root_dir.join("hctui.toml")).unwrap();
    assert!(manifest.contains("\"exec\""), "{manifest}");
    record.grants = Some(vec!["state".to_string()]);
    store.save_record(&record).unwrap();

    let mut app = app(config);
    app.sync_package_modules();
    app.sync_mod_surfaces();
    assert!(app.mods.granted(PACKAGE, "state"));
    assert!(!app.mods.granted(PACKAGE, "exec"));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn a_legacy_mod_only_install_still_renders() {
    let root = unique_dir("legacy");
    let config = config_in(&root);
    let source = root.join("source");
    std::fs::create_dir_all(source.join("dist")).unwrap();
    std::fs::write(
        source.join("hctui.toml"),
        "[mod]\nid = \"legacy-probe\"\ntitle = \"Legacy\"\nplace = \"right\"\nsize = 24\nhost_api = 1\n",
    )
    .unwrap();
    let tsx = "import { mount, Text, View } from \"nsh\";\nmount(() => <View><Text>legacy-module-ready</Text></View>);";
    let code = nsh_bundler::compile_module("App.tsx", tsx, &[]).unwrap();
    std::fs::write(source.join("dist").join("bundle.js"), code).unwrap();
    crate::mods::store::install_local(&config, &source).unwrap();

    // 旧记录：没有 capabilities 与 module，读清单回填。
    let store = Store::open(config.mods_state_dir());
    let mut record = store.record("legacy-probe").unwrap();
    record.capabilities = None;
    record.module = None;
    record.grants = None;
    store.save_record(&record).unwrap();

    let mut app = app(config);
    app.sync_package_modules();
    app.sync_mod_surfaces();
    assert!(app.chrome.mods().iter().any(|(id, _)| id == "legacy-probe"));
    assert!(app.mods.surfaces.iter().any(|surface| {
        surface.mod_id == "legacy-probe" && surface.place == crate::mods::SurfacePlace::Region
    }));
    let screen = frames(&mut app, 40);
    assert!(screen.contains("legacy-module-ready"), "screen: {screen}");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn revoking_a_grant_notifies_the_surface_and_denies_calls() {
    const TSX: &str = r#"
import { host, mount, signal, Text, View } from "nsh";
const status = signal("idle");
const change = signal("");
async function run() {
  try {
    await host.call("exec", { argv: ["echo", "hi"] });
    status.set("done");
  } catch (err) {
    status.set("rejected:" + err.message);
  }
}
host.on("grant.changed", () => change.set("grant.changed"));
host.on("state.changed", run);
mount(() => <View><Text>{change()}</Text><Text>{status()}</Text></View>);
run();
"#;

    let root = unique_dir("revoke");
    let config = config_in(&root);
    let dir = config.mods_dir().join("probe");
    std::fs::create_dir_all(dir.join("dist")).unwrap();
    std::fs::write(
        dir.join("hctui.toml"),
        "[mod]\nid = \"probe\"\nhost_api = 1\n\n[grants]\nhost = [\"exec\"]\nevents = [\"grant.changed\", \"state.changed\"]\n",
    )
    .unwrap();
    let code = nsh_bundler::compile_module("App.tsx", TSX, &[]).unwrap();
    std::fs::write(dir.join("dist").join("bundle.js"), code).unwrap();
    std::fs::write(
        dir.join("dist").join("capabilities.json"),
        "{\"host\":[\"exec\"]}",
    )
    .unwrap();

    let mut app = app(config);
    app.mods_open(
        "probe",
        crate::mods::SurfacePlace::Dialog,
        json!({}),
        Some((60, 10)),
        None,
        None,
    )
    .unwrap();
    let screen = frames(&mut app, 40);
    assert!(screen.contains("done"), "screen: {screen}");

    app.execute_action(
        "mods.grant",
        json!({"name": "probe", "host": ["exec"], "revoke": true}),
    )
    .unwrap();
    let screen = frames(&mut app, 40);
    assert!(screen.contains("grant.changed"), "screen: {screen}");
    assert!(!app.mods.granted("probe", "exec"));

    app.send_mod_event("probe", "state.changed", &json!({}));
    let screen = frames(&mut app, 40);
    assert!(screen.contains("rejected:"), "screen: {screen}");
    assert!(!screen.contains("done"), "screen: {screen}");
    std::fs::remove_dir_all(root).ok();
}
