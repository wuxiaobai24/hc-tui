//! `hc-tui notify` 的命令行契约：没有窗口时退出码 3 并带 `core_unavailable`，
//! 非法参数按用法错误退出 2（见 docs/cli-errors.md）。
//! 整段在隔离的 XDG 下跑，不碰真实配置与状态。

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

mod support;

const BIN: &str = env!("CARGO_BIN_EXE_hc-tui");

fn root(tag: &str) -> PathBuf {
    // 控制 socket 路径上限 104 字节（macOS）：`env::temp_dir()` 在 macOS 上是
    // `/var/folders/…/T`，加 `state/control-<hash>.sock` 就超限，所以用例起短根。
    let unique = format!("{}-{}", std::process::id(), support::serial());
    let root = std::path::Path::new("/tmp").join(format!("hctui-notify-cli-{tag}-{unique}"));
    std::fs::create_dir_all(root.join("config").join("hc-tui")).unwrap();
    std::fs::write(root.join("config").join("hc-tui").join("config.toml"), "").unwrap();
    std::fs::create_dir_all(root.join("state")).unwrap();
    std::fs::create_dir_all(root.join("cache")).unwrap();
    root
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(BIN)
        .args(args)
        .env("HC_SESSION", "notify-cli-test")
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .output()
        .unwrap()
}

#[test]
fn no_window_exits_three_and_says_why() {
    let root = root("no-window");
    let output = run(&root, &["notify", "build done", "--level", "error"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    println!("exit={:?} stderr={stderr:?}", output.status.code());
    assert_eq!(output.status.code(), Some(3));
    assert!(stderr.contains("notification not delivered"), "{stderr:?}");
    assert!(
        stderr.contains("no place to keep the notification"),
        "{stderr:?}"
    );
    assert!(
        stderr.trim_end().ends_with("(code: core_unavailable)"),
        "{stderr:?}"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn bad_input_is_a_usage_error() {
    let root = root("bad-input");
    for (args, expected) in [
        (vec!["notify", "x", "--level", "fatal"], "unknown level"),
        (vec!["notify", ""], "notify requires a text"),
        (vec!["notify", "a", "b"], "exactly one text argument"),
        (vec!["notify", "x", "--action"], "needs a value"),
    ] {
        let output = run(&root, &args);
        let stderr = String::from_utf8_lossy(&output.stderr);
        println!("{args:?} exit={:?} stderr={stderr:?}", output.status.code());
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(stderr.contains(expected), "{args:?}: {stderr:?}");
    }
    std::fs::remove_dir_all(root).ok();
}
