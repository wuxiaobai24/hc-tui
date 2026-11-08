//! `hc-tui notify` 的命令行契约：没有窗口时退出码 3 并说明，非法参数退出码 1。
//! 整段在隔离的 XDG 下跑，不碰真实配置与状态。

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_hc-tui");

fn root(tag: &str) -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    let root = std::env::temp_dir().join(format!("hctui-notify-cli-{tag}-{unique}"));
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
    assert!(
        stderr.contains("notification not delivered (exit 3)"),
        "{stderr:?}"
    );
    assert!(
        stderr.contains("no place to keep the notification"),
        "{stderr:?}"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn bad_input_exits_one() {
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
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert!(stderr.contains(expected), "{args:?}: {stderr:?}");
    }
    std::fs::remove_dir_all(root).ok();
}
