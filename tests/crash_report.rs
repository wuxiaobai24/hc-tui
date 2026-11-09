//! 崩溃报告：三条出口固定，模块异常带 `(模块 id, 方法, 调用序号)` 三层上下文。
//!
//! 整段在隔离的 XDG 下跑；真的 panic 一次，报告由 panic 钩子写进日志文件，
//! 断言与证据都读那份日志。

use std::path::PathBuf;

mod support;

const APP_TSX: &str = r#"
import { host, mount, Text, View } from "nsh";

async function refresh() {
  try {
    await host.call("state", { domains: ["mods"] });
  } catch (_error) {
    // 授予未到位时保持上一帧。
  }
}

refresh();
mount(() => <View><Text>probe</Text></View>);
"#;

fn isolated_state(tag: &str) -> PathBuf {
    let root = support::unique_dir("hctui-crash", tag);
    std::fs::create_dir_all(root.join("config").join("hc-tui")).unwrap();
    std::fs::write(root.join("config").join("hc-tui").join("config.toml"), "").unwrap();
    std::fs::create_dir_all(root.join("state")).unwrap();
    std::env::set_var("XDG_CONFIG_HOME", root.join("config"));
    std::env::set_var("XDG_STATE_HOME", root.join("state"));
    root
}

/// 一个真的会调用 `host.call("state")` 的模块目录。
fn module_dir(root: &std::path::Path) -> PathBuf {
    let dir = root.join("probe");
    std::fs::create_dir_all(dir.join("dist")).unwrap();
    std::fs::write(
        dir.join("hctui.toml"),
        "[mod]\nid = \"probe\"\nhost_api = 1\n\n[grants]\nhost = [\"state\"]\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("nsh.json"),
        r#"{"name": "Probe", "id": "dev.hctui.mods.probe", "version": "1.0.0", "build": 1, "capabilities": {"host": ["state"]}}"#,
    )
    .unwrap();
    // `Bundle::read` 从同目录的 `capabilities.json` 取声明的授予；没有它，运行时
    // 会拒绝 `host.call`，调用就不会到达宿主。
    std::fs::write(
        dir.join("dist").join("capabilities.json"),
        r#"{"network":{"hosts":[],"http":[]},"process":{"execute":[],"exit":false,"env":[]},"fs":{"read":[],"write":[]},"statusBar":false,"clipboard":false,"picker":false,"host":["state"],"openUrl":false}"#,
    )
    .unwrap();
    let code = nsh_bundler::compile_module("App.tsx", APP_TSX, &[]).unwrap();
    std::fs::write(dir.join("dist").join("bundle.js"), code).unwrap();
    dir
}

#[test]
fn a_panic_reports_the_three_exits_and_the_module_call_context() {
    let root = isolated_state("report");
    let config = hc_tui::config::Config::load(None);
    let dir = module_dir(&root);
    hc_tui::diag::clear_call_context();
    hc_tui::mods::check::check(&config, dir.to_str().unwrap()).unwrap();
    for _ in 0..50 {
        if !hc_tui::diag::call_context().is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(
        !hc_tui::diag::call_context().is_empty(),
        "the module's host.call should be recorded"
    );

    hc_tui::diag::install();
    let handle = std::thread::Builder::new()
        .name("hc-engine".to_string())
        .spawn(|| panic!("module exploded"))
        .unwrap();
    assert!(handle.join().is_err(), "the thread should have panicked");

    let log = hc_tui::diag::log_path();
    let text = std::fs::read_to_string(&log).unwrap();
    println!("{text}");
    for expected in [
        "崩溃点: ",
        "线程: hc-engine",
        "消息: module exploded",
        "模块调用上下文",
        "(probe, state, 1)",
        "日志文件: ",
        "复现命令: RUST_BACKTRACE=1 hc-tui",
        hc_tui::diag::ISSUE_URL,
    ] {
        assert!(text.contains(expected), "missing {expected:?} in:\n{text}");
    }
    std::fs::remove_dir_all(root).ok();
}
