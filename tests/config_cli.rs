//! `config check` 与 `config reset-keys`：坏配置指到行列、键位冲突只警告、
//! 重置只动 `[keys]` 段。整段在隔离的 XDG 下跑，不碰真实配置与状态。

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

mod support;

const BIN: &str = env!("CARGO_BIN_EXE_hc-tui");

/// 一个隔离的 XDG 根；测试进程之间不共享配置与状态。
fn root(tag: &str) -> PathBuf {
    let root = support::unique_dir("hctui-config-cli", tag);
    std::fs::create_dir_all(root.join("config").join("hc-tui")).unwrap();
    std::fs::write(root.join("config").join("hc-tui").join("config.toml"), "").unwrap();
    std::fs::create_dir_all(root.join("state")).unwrap();
    std::fs::create_dir_all(root.join("cache")).unwrap();
    root
}

fn config_dir(root: &Path) -> PathBuf {
    root.join("config").join("hc-tui")
}

fn write(root: &Path, name: &str, text: &str) -> PathBuf {
    let path = config_dir(root).join(name);
    std::fs::write(&path, text).unwrap();
    path
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(BIN)
        .args(args)
        .env("HC_SESSION", "config-cli-test")
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .output()
        .unwrap()
}

#[test]
fn bad_configs_point_at_file_line_and_column() {
    let root = root("bad");
    let cases = [
        ("quote.toml", "[theme]\nname = \"nord\n", ":2:8:"),
        ("key.toml", "[ui]\nsiderbar = false\n", ":2:1:"),
        (
            "dup.toml",
            "[keys]\nnew_tab = \"prefix+t\"\nnew_tab = \"prefix+y\"\n",
            ":3:11:",
        ),
    ];
    for (name, text, expected) in cases {
        let path = write(&root, name, text);
        let output = run(
            &root,
            &["config", "check", "--config", path.to_str().unwrap()],
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(output.status.code(), Some(1), "{name}: {stdout}");
        assert!(stdout.contains(expected), "{name}: {stdout}");
    }
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn key_conflicts_warn_without_failing() {
    let root = root("conflict");
    let path = write(&root, "conflict.toml", "[keys]\nnew_tab = \"prefix+q\"\n");
    let output = run(
        &root,
        &["config", "check", "--config", path.to_str().unwrap()],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    // 退出码契约（docs/cli-errors.md）：2 只表示用法错误，所以只有警告时退出 0。
    assert_eq!(output.status.code(), Some(0), "{stdout}");
    assert!(
        stdout.contains("键位冲突 prefix+q：detach、new_tab"),
        "{stdout}"
    );

    // 干净配置（`--default-config` 的原样输出）退出 0：默认值里已知的重键不算。
    let default = run(&root, &["--default-config"]);
    let path = write(
        &root,
        "clean.toml",
        &String::from_utf8_lossy(&default.stdout),
    );
    let output = run(
        &root,
        &["config", "check", "--config", path.to_str().unwrap()],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(0), "{stdout}");
    assert!(stdout.contains("主题 default"), "{stdout}");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn reset_keys_keeps_other_sections_and_backs_up() {
    let root = root("reset");
    let original = "[ui]\nsidebar = false\n\n[keys]\nnew_tab = \"prefix+q\"\npreset = \"zellij\"\n\n[theme]\nname = \"nord\"\n";
    let path = write(&root, "config.toml", original);

    // 不带 --yes 只打印计划，文件与目录都不动。
    let output = run(
        &root,
        &["config", "reset-keys", "--config", path.to_str().unwrap()],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    assert!(stdout.contains("未写入"), "{stdout}");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    assert!(backups(&root).is_empty());

    let output = run(
        &root,
        &[
            "config",
            "reset-keys",
            "--config",
            path.to_str().unwrap(),
            "--yes",
        ],
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{stdout}");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("sidebar = false"), "{text}");
    assert!(text.contains("name = \"nord\""), "{text}");
    assert!(text.contains("new_tab = \"prefix+c\""), "{text}");
    assert!(!text.contains("new_tab = \"prefix+q\""), "{text}");

    let backups = backups(&root);
    assert_eq!(backups.len(), 1, "{backups:?}");
    assert_eq!(std::fs::read_to_string(&backups[0]).unwrap(), original);
    std::fs::remove_dir_all(root).ok();
}

fn backups(root: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(config_dir(root))
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().contains(".bak."))
        })
        .collect();
    found.sort();
    found
}
