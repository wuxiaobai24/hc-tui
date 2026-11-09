//! `packages` / `plugins` / `mods` 三个表面的用法输出：`--help` 打印用法并以 0 退出，
//! 未知子命令带用法并以非 0 退出。整段在隔离的 XDG 下跑，不碰真实配置与状态。

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

mod support;

const BIN: &str = env!("CARGO_BIN_EXE_hc-tui");
const SURFACES: [&str; 3] = ["packages", "plugins", "mods"];

/// 一个隔离的 XDG 根；测试进程之间不共享配置与状态。
fn root(tag: &str) -> PathBuf {
    let root = support::unique_dir("hctui-cli-usage", tag);
    std::fs::create_dir_all(root.join("config").join("hc-tui")).unwrap();
    std::fs::write(root.join("config").join("hc-tui").join("config.toml"), "").unwrap();
    std::fs::create_dir_all(root.join("state")).unwrap();
    std::fs::create_dir_all(root.join("cache")).unwrap();
    root
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(BIN)
        .args(args)
        .env("HC_SESSION", "cli-usage-test")
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .output()
        .unwrap()
}

#[test]
fn help_prints_the_usage_and_exits_zero() {
    let root = root("help");
    for surface in SURFACES {
        for args in [
            vec![surface, "--help"],
            vec![surface, "-h"],
            vec![surface, "list", "--help"],
            vec![surface, "install", "--help"],
        ] {
            let output = run(&root, &args);
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(
                output.status.success(),
                "{args:?} exited {:?}: {}",
                output.status.code(),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                stdout.contains(&format!("usage: hc-tui {surface} ")),
                "{args:?} printed {stdout:?}"
            );
        }
    }
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn an_unknown_subcommand_exits_non_zero_with_the_usage() {
    let root = root("unknown");
    for surface in SURFACES {
        let output = run(&root, &[surface, "definitely-not-a-subcommand"]);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "{surface} accepted an unknown subcommand"
        );
        assert!(
            stderr.contains(&format!("usage: hc-tui {surface} ")),
            "{surface} printed {stderr:?}"
        );
        assert!(
            stderr.contains("unknown: definitely-not-a-subcommand"),
            "{surface} printed {stderr:?}"
        );
    }
    std::fs::remove_dir_all(root).ok();
}
