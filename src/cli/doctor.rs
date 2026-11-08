//! `hc-tui doctor`：一次跑完的分节自检。
//!
//! 每节固定 `[大写节名]: 值`。失败时写 `FAILED — 原因`，紧跟 `已查找:` 与
//! `hint:` 两行；缺项写 `Not Found`。全 ok 退出 0，有失败退出 1。

use std::path::PathBuf;

use anyhow::Result;
use hc_tui::config::Config;
use hc_tui::error::{reported, usage, ErrorCode, Exit};

use crate::wants_help;

/// 用法行；`--help` 与未知选项共用。
fn usage_line() -> &'static str {
    "usage: hc-tui doctor [--config <path>]"
}

pub fn run(args: &[String]) -> Result<()> {
    if args.iter().any(|arg| wants_help(arg)) {
        return crate::print_help("doctor");
    }
    let mut path = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--config" => {
                index += 1;
                path = Some(PathBuf::from(
                    args.get(index)
                        .ok_or_else(|| usage("--config needs a value"))?,
                ));
            }
            other => return Err(usage(format!("{} (unknown: {})", usage_line(), other))),
        }
        index += 1;
    }

    let config = Config::load(path.clone());
    let mut code = None;
    if config_section(&config, path.as_deref()) > 0 {
        code = Some(ErrorCode::ConfigInvalid);
    }
    if store_section(&config) > 0 {
        code = code.or(Some(ErrorCode::ManifestInvalid));
    }
    core_section(&config);
    println!("[LOGGER]: {}", hc_tui::diag::log_path().display());
    println!(
        "[VERSION]: hc-tui v{} ({})",
        env!("CARGO_PKG_VERSION"),
        env!("HC_TUI_TARGET")
    );
    if let Some(code) = code {
        // 分节报告在 stdout，码也写在 stdout，保证「末行是码」在单一流里成立。
        println!("(code: {})", code.as_str());
        return Err(reported(Exit::Failure));
    }
    Ok(())
}

/// 配置文件查找过的目录：`--config` 的父目录优先，其次是 XDG 默认目录。
fn config_dirs(config: &Config, explicit: Option<&std::path::Path>) -> Vec<String> {
    let mut dirs: Vec<String> = Vec::new();
    let mut push = |path: PathBuf| {
        let text = path.display().to_string();
        if !dirs.contains(&text) {
            dirs.push(text);
        }
    };
    if let Some(path) = explicit {
        if let Some(parent) = path.parent() {
            push(parent.to_path_buf());
        }
    }
    if let Some(parent) = config.config_path.parent() {
        push(parent.to_path_buf());
    }
    if dirs.is_empty() {
        dirs.push("Not Found".to_string());
    }
    dirs
}

/// `[CONFIG]` / `[CONFIG CHECK]` / `[CONFIG DIRS]`；返回失败节数。
fn config_section(config: &Config, explicit: Option<&std::path::Path>) -> usize {
    let path = &config.config_path;
    let dirs = config_dirs(config, explicit);
    println!("[CONFIG]: {}", path.display());
    let failed = if config.errors.is_empty() {
        println!("[CONFIG CHECK]: ok");
        0
    } else {
        let first = &config.errors[0];
        println!(
            "[CONFIG CHECK]: FAILED — {} 处非法（{}:{}:{}: {}）",
            config.errors.len(),
            path.display(),
            first.line,
            first.column,
            first.message
        );
        println!("已查找: {}", dirs.join(", "));
        println!(
            "hint: cp {} {}.bak && hc-tui --default-config > {}",
            path.display(),
            path.display(),
            path.display()
        );
        1
    };
    println!("[CONFIG DIRS]: {}", dirs.join(", "));
    failed
}

/// `[PACKAGE STORE]` / `[STORE CHECK]`；返回失败节数。
fn store_section(config: &Config) -> usize {
    let mut packages = 0;
    let mut enabled = 0;
    let mut problems: Vec<String> = Vec::new();
    let mut searched: Vec<String> = Vec::new();
    for state in [config.plugins_state_dir(), config.mods_state_dir()] {
        let store = hc_tui::store::Store::open(state.clone());
        for record in store.list() {
            packages += 1;
            if record.enabled {
                enabled += 1;
            }
        }
        let generations = state.join("store");
        searched.push(generations.display().to_string());
        let records = state.join("records");
        searched.push(records.display().to_string());
        for record in files(&records) {
            let parsed = std::fs::read_to_string(&record)
                .map_err(|error| error.to_string())
                .and_then(|text| {
                    serde_json::from_str::<hc_tui::store::Record>(&text)
                        .map(|_| ())
                        .map_err(|error| error.to_string())
                });
            if let Err(error) = parsed {
                problems.push(format!("{}: {error}", record.display()));
            }
        }
        for package in subdirs(&generations) {
            for generation in subdirs(&package) {
                if !generation.join("hctui.toml").is_file()
                    && !generation.join("plugin.toml").is_file()
                {
                    continue;
                }
                if let Err(error) = hc_tui::pkg::load_dir(&generation) {
                    problems.push(error.to_string());
                }
            }
        }
    }
    if packages == 0 {
        println!("[PACKAGE STORE]: Not Found");
    } else {
        println!("[PACKAGE STORE]: {packages} 个包 / {enabled} 启用");
    }
    if problems.is_empty() {
        println!("[STORE CHECK]: ok");
        return 0;
    }
    println!("[STORE CHECK]: FAILED — {}", problems[0]);
    println!("已查找: {}", searched.join(", "));
    println!("hint: hc-tui packages uninstall <id> && hc-tui packages install <source>");
    1
}

/// `[CORE]`：只探测已存在的 socket，不按需拉起。
fn core_section(config: &Config) {
    match hc_tui::app::winless::core_socket_path(config) {
        None => println!("[CORE]: 未运行（XDG_RUNTIME_DIR 未设置）"),
        Some(path) => {
            if hc_tui::app::winless::core_alive(config) {
                println!("[CORE]: 运行中（{}）", path.display());
            } else {
                println!("[CORE]: 未运行（socket 不存在 {}）", path.display());
            }
        }
    }
}

/// 目录下的一级子目录，按名字排序；读不到时为空。
fn subdirs(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = entries(dir)
        .into_iter()
        .filter(|path| path.is_dir())
        .collect();
    found.sort();
    found
}

/// 目录下的一级文件，按名字排序；读不到时为空。
fn files(dir: &std::path::Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = entries(dir)
        .into_iter()
        .filter(|path| path.is_file())
        .collect();
    found.sort();
    found
}

fn entries(dir: &std::path::Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect()
}
