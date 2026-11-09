//! M9f 五个补动作的真实调用：成功路径与参数非法各走一遍。
//!
//! 走 [`App::execute_action`] 的正式分发，不 mock：成功返回 `Ok` 并排进 core
//! RPC，非法参数在进入 RPC 之前就报确定文案。

use hc_tui::app::App;
use hc_tui::config::Config;
use serde_json::json;

mod support;

fn app() -> App {
    let root = support::unique_dir("hctui-m9f-actions", "app");
    let config = Config {
        restore: false,
        prompt_new_tab: false,
        session: Some(format!("hcp-m9f-actions-{}", std::process::id())),
        config_path: root.join("config").join("config.toml"),
        state_path: root.join("state").join("state.json"),
        ..Config::default()
    };
    App::new(config).expect("app builds")
}

fn accepted(app: &mut App, name: &str, args: serde_json::Value, label: &str) {
    let result = app.execute_action(name, args);
    assert!(result.is_ok(), "{name} should accept {label}: {result:?}");
    println!("{name} accepted {label}");
}

fn rejected(app: &mut App, name: &str, args: serde_json::Value, need: &str) {
    let error = app
        .execute_action(name, args)
        .expect_err(&format!("{name} should reject"));
    println!("{name} rejected: {error}");
    assert!(
        error.to_string().contains(need),
        "{name} error {error:?} misses {need:?}"
    );
}

#[test]
fn five_actions_accept_and_reject() {
    let mut app = app();
    let terminal = "local/t1";

    accepted(
        &mut app,
        "terminal.send",
        json!({"terminal": terminal, "text": "hello"}),
        "terminal+text",
    );
    rejected(
        &mut app,
        "terminal.send",
        json!({"terminal": terminal}),
        "text required",
    );

    accepted(
        &mut app,
        "terminal.keys",
        json!({"terminal": terminal, "keys": ["enter", "ctrl+c"]}),
        "terminal+keys",
    );
    rejected(
        &mut app,
        "terminal.keys",
        json!({"terminal": terminal, "keys": []}),
        "keys required",
    );

    accepted(
        &mut app,
        "terminal.wait",
        json!({"terminal": terminal, "output": "ready"}),
        "terminal+output",
    );
    rejected(
        &mut app,
        "terminal.wait",
        json!({"terminal": terminal}),
        "output required",
    );

    let file = support::unique_dir("hctui-m9f-put", "fixture").with_extension("bin");
    std::fs::write(&file, [0u8, 159, 146, 150]).expect("fixture");
    accepted(
        &mut app,
        "terminal.put",
        json!({"terminal": terminal, "path": file.to_string_lossy()}),
        "terminal+binary path",
    );
    rejected(
        &mut app,
        "terminal.put",
        json!({"terminal": terminal, "path": "/definitely/missing/file"}),
        "read failed",
    );
    std::fs::remove_file(&file).ok();

    accepted(
        &mut app,
        "machine.rename",
        json!({"machine": "peer", "name": "box"}),
        "machine+name",
    );
    rejected(
        &mut app,
        "machine.rename",
        json!({"machine": "peer"}),
        "name required",
    );
}
