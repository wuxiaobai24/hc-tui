//! 三段式错误与 `doctor` 分节自检。
//!
//! 整段在隔离的 XDG 下跑：三条常见错误（找不到包 / 清单非法 / 配置非法）都要
//! 回显输入、给出 `Possible fix:`；`doctor` 的全 ok、core 未运行、包清单坏掉
//! 三种状态各断一次。

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

mod support;

const BIN: &str = env!("CARGO_BIN_EXE_hc-tui");

fn root(tag: &str) -> PathBuf {
    let root = support::unique_dir("hctui-diag-cli", tag);
    std::fs::create_dir_all(root.join("config").join("hc-tui")).unwrap();
    std::fs::write(root.join("config").join("hc-tui").join("config.toml"), "").unwrap();
    std::fs::create_dir_all(root.join("state")).unwrap();
    std::fs::create_dir_all(root.join("cache")).unwrap();
    root
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(BIN)
        .args(args)
        .env("HC_SESSION", "diag-cli-test")
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env_remove("HCTUI_SOCKET")
        .output()
        .unwrap()
}

fn both(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn a_missing_package_echoes_the_token_and_lists_every_searched_dir() {
    let root = root("missing");
    let output = run(&root, &["mods", "check", "definitely-absent"]);
    let text = both(&output);
    assert_eq!(output.status.code(), Some(1), "{text}");
    assert!(text.contains("找不到模块 definitely-absent"), "{text}");
    assert!(text.contains("你的情况："), "{text}");
    assert!(
        text.contains("Possible fix: hc-tui packages list"),
        "{text}"
    );
    assert!(
        text.contains("config/hc-tui/mods/definitely-absent"),
        "{text}"
    );
    assert!(
        text.contains("state/hc-tui/mods/store/definitely-absent/"),
        "{text}"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn a_broken_manifest_points_at_the_line_and_the_package() {
    let root = root("manifest");
    let package = root.join("package");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(package.join("hctui.toml"), "[mod]\nid = \"probe\n").unwrap();
    let output = run(&root, &["mods", "check", package.to_str().unwrap()]);
    let text = both(&output);
    assert_eq!(output.status.code(), Some(1), "{text}");
    assert!(
        text.contains(&format!("{}: 清单非法", package.display())),
        "{text}"
    );
    // 三个数字是行、列与原因里的行号都不是唯一标准，只断关键词。
    assert!(text.contains("hctui.toml:2:"), "{text}");
    assert!(text.contains("Possible fix: sed -n"), "{text}");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn a_broken_config_names_the_file_and_offers_a_pasteable_fix() {
    let root = root("config");
    let path = root.join("config").join("hc-tui").join("config.toml");
    std::fs::write(&path, "[ui]\nsiderbar = false\n").unwrap();
    let output = run(&root, &["config", "check"]);
    let text = both(&output);
    assert_eq!(output.status.code(), Some(1), "{text}");
    assert!(
        text.contains(&format!("配置文件非法 {}", path.display())),
        "{text}"
    );
    assert!(text.contains(":2:1:"), "{text}");
    assert!(text.contains("Possible fix: cp "), "{text}");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn doctor_sections_report_ok_and_the_missing_core() {
    let root = root("doctor-ok");
    let output = run(&root, &["doctor"]);
    let text = both(&output);
    assert_eq!(output.status.code(), Some(0), "{text}");
    for section in [
        "[CONFIG]: ",
        "[CONFIG CHECK]: ok",
        "[CONFIG DIRS]: ",
        "[STORE CHECK]: ok",
        "[CORE]: ",
        "[LOGGER]: ",
        "[VERSION]: hc-tui v",
    ] {
        assert!(text.contains(section), "missing {section:?} in:\n{text}");
    }
    assert!(text.contains("[CORE]: 未运行"), "{text}");
    assert!(text.contains("[PACKAGE STORE]: Not Found"), "{text}");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn doctor_fails_when_a_package_manifest_is_broken() {
    let root = root("doctor-store");
    let generation = root
        .join("state")
        .join("hc-tui")
        .join("mods")
        .join("store")
        .join("beta")
        .join("2");
    std::fs::create_dir_all(&generation).unwrap();
    std::fs::write(
        generation.join("hctui.toml"),
        "manifest_version = 2\nid = \"beta\"\ntitel = \"Beta\"\n",
    )
    .unwrap();
    let output = run(&root, &["doctor"]);
    let text = both(&output);
    assert_eq!(output.status.code(), Some(1), "{text}");
    assert!(text.contains("[STORE CHECK]: FAILED — "), "{text}");
    assert!(text.contains("unknown field `titel`"), "{text}");
    assert!(text.contains("已查找: "), "{text}");
    assert!(text.contains("hint: hc-tui packages uninstall"), "{text}");
    std::fs::remove_dir_all(root).ok();
}
