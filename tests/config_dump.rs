//! `config dump|init`：默认配置与仓库文件同源、导出可往返、`--minimal` 只出差异、
//! `init` 不覆盖已有文件。整段在隔离的 XDG 下跑，不碰真实配置与状态。

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

mod support;

const BIN: &str = env!("CARGO_BIN_EXE_hc-tui");

/// 一个隔离的 XDG 根；测试进程之间不共享配置与状态。
fn root(tag: &str) -> PathBuf {
    let root = support::unique_dir("hctui-config-dump", tag);
    std::fs::create_dir_all(root.join("config").join("hc-tui")).unwrap();
    std::fs::write(root.join("config").join("hc-tui").join("config.toml"), "").unwrap();
    std::fs::create_dir_all(root.join("state")).unwrap();
    std::fs::create_dir_all(root.join("cache")).unwrap();
    root
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(BIN)
        .args(args)
        .env("HC_SESSION", "config-dump-test")
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .output()
        .unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

/// `docs/default-config.toml` 必须等于 `config dump --defaults`；过期就让测试红。
/// 重新生成：`HC_UPDATE_DEFAULT_CONFIG=1 cargo test -- config_default_file_is_current`
#[test]
fn config_default_file_is_current() {
    let root = root("defaults");
    let output = run(&root, &["config", "dump", "--defaults"]);
    assert!(output.status.success(), "{}", stdout(&output));
    let dumped = stdout(&output);
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/default-config.toml");
    if std::env::var_os("HC_UPDATE_DEFAULT_CONFIG").is_some() {
        std::fs::write(&path, &dumped).unwrap();
        std::fs::remove_dir_all(root).ok();
        return;
    }
    let current = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        current, dumped,
        "docs/default-config.toml 与 `config dump --defaults` 不一致；\
         运行 `HC_UPDATE_DEFAULT_CONFIG=1 cargo test -- config_default_file_is_current` 重新生成"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn dump_roundtrips_through_check() {
    let root = root("roundtrip");
    let source = root.join("source.toml");
    std::fs::write(&source, "[render]\nfps = 90\n").unwrap();
    let first = root.join("dumped.toml");
    let output = run(
        &root,
        &[
            "config",
            "dump",
            "--config",
            source.to_str().unwrap(),
            "--out",
            first.to_str().unwrap(),
        ],
    );
    assert!(output.status.success(), "{}", stdout(&output));
    let output = run(
        &root,
        &["config", "check", "--config", first.to_str().unwrap()],
    );
    assert_eq!(output.status.code(), Some(0), "{}", stdout(&output));
    let second = run(
        &root,
        &["config", "dump", "--config", first.to_str().unwrap()],
    );
    assert!(second.status.success(), "{}", stdout(&second));
    assert_eq!(
        std::fs::read_to_string(&first).unwrap(),
        stdout(&second),
        "config dump 往返后文本变了"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn minimal_lists_only_changed_keys() {
    let root = root("minimal");
    let path = root.join("config").join("hc-tui").join("config.toml");
    std::fs::write(&path, "[render]\nfps = 90\n").unwrap();
    let output = run(&root, &["config", "dump", "--minimal"]);
    assert!(output.status.success(), "{}", stdout(&output));
    let text = stdout(&output);
    let keys: Vec<&str> = text
        .lines()
        .filter(|line| !line.starts_with('#') && !line.starts_with('['))
        .filter(|line| !line.trim().is_empty())
        .collect();
    assert_eq!(keys, vec!["fps = 90"], "{text}");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn init_refuses_to_overwrite_and_backs_up_on_force() {
    let root = root("init");
    let target = root.join("config").join("hc-tui").join("config.toml");
    std::fs::write(&target, "user = 1\n").unwrap();

    let output = run(
        &root,
        &["config", "init", "--path", target.to_str().unwrap()],
    );
    assert!(!output.status.success(), "{}", stdout(&output));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("已存在"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "user = 1\n");

    let output = run(
        &root,
        &[
            "config",
            "init",
            "--path",
            target.to_str().unwrap(),
            "--force",
        ],
    );
    assert!(output.status.success(), "{}", stdout(&output));
    let backups: Vec<PathBuf> = std::fs::read_dir(root.join("config").join("hc-tui"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().contains(".bak."))
        })
        .collect();
    assert_eq!(backups.len(), 1, "{backups:?}");
    assert_eq!(std::fs::read_to_string(&backups[0]).unwrap(), "user = 1\n");
    let defaults = run(&root, &["config", "dump", "--defaults"]);
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        stdout(&defaults),
        "init 写出的内容必须等于 dump --defaults"
    );
    std::fs::remove_dir_all(root).ok();
}
