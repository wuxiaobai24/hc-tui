//! CLI 失败契约：稳定错误码、两种错误形状、四档退出码。
//! 整段在隔离的 XDG 下跑，不碰真实配置与状态。

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_hc-tui");

/// 一个隔离的 XDG 根；测试进程之间不共享配置与状态。
fn root(tag: &str) -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    let root = std::env::temp_dir().join(format!("hctui-cli-errors-{tag}-{unique}"));
    std::fs::create_dir_all(root.join("config").join("hc-tui")).unwrap();
    std::fs::write(root.join("config").join("hc-tui").join("config.toml"), "").unwrap();
    std::fs::create_dir_all(root.join("state")).unwrap();
    std::fs::create_dir_all(root.join("cache")).unwrap();
    root
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(BIN)
        .args(args)
        .env("HC_SESSION", "cli-errors-test")
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("XDG_STATE_HOME", root.join("state"))
        .env("XDG_CACHE_HOME", root.join("cache"))
        .output()
        .unwrap()
}

/// `--json` 失败的 stderr 应当是一个对象：`code` 与 `id` 都能取出来。
fn json_error(output: &Output) -> (String, String) {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let value: serde_json::Value =
        serde_json::from_str(stderr.trim()).unwrap_or_else(|error| panic!("{error}: {stderr:?}"));
    (
        value["error"]["code"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        value["id"].as_str().unwrap_or_default().to_string(),
    )
}

#[test]
fn a_usage_error_exits_2_with_the_stable_code() {
    let root = root("usage");
    let output = run(&root, &["packages", "list", "--json", "--nope"]);
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        json_error(&output),
        ("usage".to_string(), "cli:packages:list".to_string())
    );
    assert!(output.stdout.is_empty());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn a_missing_package_echoes_the_token_and_exits_1() {
    let root = root("missing");
    let output = run(
        &root,
        &["packages", "update", "--check", "--json", "no.such.pkg"],
    );
    let (code, id) = json_error(&output);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(code, "package_not_found");
    assert_eq!(id, "cli:packages:update");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("no.such.pkg"), "{stderr}");

    // 人类形态：末行固定是码。
    let output = run(&root, &["packages", "update", "--check", "no.such.pkg"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr.trim_end().ends_with("(code: package_not_found)"),
        "{stderr}"
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn a_broken_manifest_is_manifest_invalid() {
    let root = root("manifest");
    let package = root.join("bad");
    std::fs::create_dir_all(&package).unwrap();
    std::fs::write(
        package.join("hctui.toml"),
        "manifest_version = 2\nid = \"dev.example.bad\"\nname = \"Bad\"\nversion = \"0.1.0\"\nhost_api = 1\nunknown_key = 3\n",
    )
    .unwrap();
    let output = run(
        &root,
        &[
            "packages",
            "install",
            "--json",
            "--yes",
            package.to_str().unwrap(),
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(json_error(&output).0, "manifest_invalid");
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn no_window_is_core_unavailable_and_exits_3() {
    let root = root("nocore");
    let output = run(&root, &["plugins", "action", "invoke", "--json", "nope"]);
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(
        json_error(&output),
        (
            "core_unavailable".to_string(),
            "cli:plugins:action:invoke".to_string()
        )
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn a_broken_config_is_config_invalid_and_warnings_still_exit_0() {
    let root = root("config");
    let path = root.join("config").join("hc-tui").join("bad.toml");
    std::fs::write(&path, "[theme]\nname = \"nord\n").unwrap();
    let output = run(
        &root,
        &["config", "check", "--config", path.to_str().unwrap()],
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr.trim_end().ends_with("(code: config_invalid)"),
        "{stderr}"
    );

    std::fs::write(&path, "[keys]\nnew_tab = \"prefix+q\"\n").unwrap();
    let output = run(
        &root,
        &["config", "check", "--config", path.to_str().unwrap()],
    );
    assert_eq!(output.status.code(), Some(0));
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn the_success_path_keeps_its_json_shape() {
    let root = root("success");
    let output = run(&root, &["packages", "list", "--json"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "[]");
    assert!(output.stderr.is_empty());
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn json_needs_an_explicit_flag() {
    let root = root("human");
    let output = run(&root, &["packages", "update", "--check", "no.such.pkg"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("\n(code: package_not_found)"), "{stderr}");
    assert!(!stderr.trim_start().starts_with('{'), "{stderr}");
    std::fs::remove_dir_all(root).ok();
}
