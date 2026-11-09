//! CLI 失败契约：稳定错误码、两种错误形状、四档退出码。
//! 整段在隔离的 XDG 下跑，不碰真实配置与状态。

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

mod support;

const BIN: &str = env!("CARGO_BIN_EXE_hc-tui");

/// 一个隔离的 XDG 根；测试进程之间不共享配置与状态。
fn root(tag: &str) -> PathBuf {
    let root = support::unique_dir("hctui-cli-errors", tag);
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

#[test]
fn keys_where_prints_the_prefix_chain_and_its_own_code() {
    let root = root("keys-where");
    let help = run(&root, &["keys", "where", "help", "--json"]);
    assert_eq!(help.status.code(), Some(0));
    let value: serde_json::Value =
        serde_json::from_slice(&help.stdout).expect("keys where --json prints one object");
    assert_eq!(value["action"], serde_json::json!("help"));
    assert_eq!(
        value["keys"],
        serde_json::json!(["ctrl+b", "?"]),
        "the chain keeps the prefix key first"
    );

    let human = run(&root, &["keys", "where", "help"]);
    let stdout = String::from_utf8_lossy(&human.stdout);
    assert!(stdout.contains("→  help"), "{stdout:?}");

    let missing = run(
        &root,
        &["keys", "where", "definitely-not-an-action", "--json"],
    );
    assert_eq!(missing.status.code(), Some(1));
    assert_eq!(
        json_error(&missing),
        ("action_not_found".to_string(), "cli:keys:where".to_string())
    );
    std::fs::remove_dir_all(root).ok();
}

#[test]
fn host_api_accepts_a_range_and_rejects_a_too_new_one() {
    let root = root("host-api");
    let write = |name: &str, host_api: &str| {
        let package = root.join(name);
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(
            package.join("hctui.toml"),
            format!(
                "manifest_version = 2\nid = \"dev.example.{name}\"\nname = \"Probe\"\nversion = \"0.1.0\"\nhost_api = {host_api}\n"
            ),
        )
        .unwrap();
        package
    };
    let exact = write("exact", "1");
    let range = write("range", "\"1..=2\"");
    let too_new = write("toonew", "\"2..=3\"");

    for (package, field) in [(&exact, "1"), (&range, "1..=2")] {
        let output = run(
            &root,
            &["packages", "info", "--json", package.to_str().unwrap()],
        );
        assert_eq!(
            output.status.code(),
            Some(0),
            "{}: {}",
            package.display(),
            String::from_utf8_lossy(&output.stderr)
        );
        let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["host_api"], serde_json::json!(field));
    }

    let output = run(
        &root,
        &["packages", "info", "--json", too_new.to_str().unwrap()],
    );
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    let value: serde_json::Value = serde_json::from_str(stderr.trim()).unwrap();
    assert_eq!(
        value["error"]["code"],
        serde_json::json!("host_api_too_old")
    );
    let message = value["error"]["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("upgrade hc-tui") && message.contains("downgrade the package"),
        "{message}"
    );
    std::fs::remove_dir_all(root).ok();
}
