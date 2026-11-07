//! 插件终端对代码代际的引用。
//!
//! 终端在已卸载的插件代际里运行着包内程序时，代际目录不能在卸载时删除。
//! 每个客户端进程为每个包每个代际写一份
//! `<state>/plugins/leases/<package>.<pid>.<generation>.json`，记录仍在使用它的
//! 终端；最后一条终端关闭后删除该文件，包没有记录时回收代际。进程已退出的
//! 文件在下一次扫描时清掉。

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::config::Config;
use crate::store::Store;

fn leases_dir(config: &Config) -> PathBuf {
    config.plugins_state_dir().join("leases")
}

fn lease_path(config: &Config, package: &str, pid: u32, generation: u64) -> PathBuf {
    leases_dir(config).join(format!("{package}.{pid}.{generation}.json"))
}

fn read(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn write(path: &Path, value: &Value) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(value)?)
}

fn terminals_of(value: &Value) -> Vec<String> {
    value
        .get("terminals")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// 进程是否还在；非 Unix 上无法判断时按存活处理。
fn alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        if pid == 0 {
            return false;
        }
        // SAFETY: kill with signal 0 only checks for existence.
        let error = unsafe { libc::kill(pid as i32, 0) };
        error == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

/// 记录这个进程开始使用某个代际；`terminal` 是入口名与 Pane 编号。
pub fn acquire(
    config: &Config,
    package: &str,
    generation: u64,
    root: &Path,
    terminal: &str,
) -> std::io::Result<()> {
    let path = lease_path(config, package, std::process::id(), generation);
    let mut terminals = read(&path)
        .map(|value| terminals_of(&value))
        .unwrap_or_default();
    if !terminals.iter().any(|item| item == terminal) {
        terminals.push(terminal.to_string());
        terminals.sort();
    }
    write(
        &path,
        &json!({
            "package": package,
            "pid": std::process::id(),
            "generation": generation,
            "root": root.display().to_string(),
            "terminals": terminals,
        }),
    )
}

/// 释放一条终端引用；返回被回收的代际号。
///
/// 包已卸载（没有记录与 `current` 指针）且没有进程再引用代际时回收剩下的代际目录。
pub fn release(config: &Config, package: &str, terminal: &str) -> Vec<u64> {
    let prefix = format!("{package}.{}.", std::process::id());
    if let Ok(entries) = std::fs::read_dir(leases_dir(config)) {
        for entry in entries.filter_map(Result::ok) {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with(&prefix) || !name.ends_with(".json") {
                continue;
            }
            let Some(mut value) = read(&entry.path()) else {
                continue;
            };
            let terminals: Vec<String> = terminals_of(&value)
                .into_iter()
                .filter(|item| item != terminal)
                .collect();
            if terminals.is_empty() {
                let _ = std::fs::remove_file(entry.path());
            } else {
                value["terminals"] = json!(terminals);
                let _ = write(&entry.path(), &value);
            }
        }
    }
    prune(config, package)
}

/// 回收不再被引用的代际：包没有记录时才有代际可回收。
pub fn prune(config: &Config, package: &str) -> Vec<u64> {
    let store = Store::open(config.plugins_state_dir());
    if store.record(package).is_some() {
        return Vec::new();
    }
    let live = live_generations(config, package);
    let removed = store.prune(package, &live);
    if live.is_empty() {
        let _ = store.remove_store(package);
    }
    removed
}

/// 仍被存活进程引用的代际；顺带清理已退出进程的引用文件。
pub fn live_generations(config: &Config, package: &str) -> Vec<u64> {
    let Ok(entries) = std::fs::read_dir(leases_dir(config)) else {
        return Vec::new();
    };
    let prefix = format!("{package}.");
    let mut generations = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with(&prefix) || !name.ends_with(".json") {
            continue;
        }
        let Some(value) = read(&entry.path()) else {
            continue;
        };
        let alive_pid = value
            .get("pid")
            .and_then(Value::as_u64)
            .map(|pid| alive(pid as u32))
            .unwrap_or(false);
        if !alive_pid {
            let _ = std::fs::remove_file(entry.path());
            continue;
        }
        if let Some(generation) = value.get("generation").and_then(Value::as_u64) {
            generations.push(generation);
        }
    }
    generations.sort_unstable();
    generations.dedup();
    generations
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Record, Source};

    fn unique_dir(tag: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        std::env::temp_dir().join(format!("hctui-plugin-lease-{tag}-{unique}"))
    }

    fn config_in(root: &Path) -> Config {
        Config {
            config_path: root.join("config").join("config.toml"),
            state_path: root.join("state").join("state.json"),
            ..Config::default()
        }
    }

    fn record(generation: u64) -> Record {
        Record {
            id: "dev.example.git".into(),
            source: Source::Local,
            generation,
            installed_at: 0,
            enabled: true,
            manifest_hash: None,
            origin: None,
            sha256: None,
            key_id: None,
            app: None,
            min_runtime: None,
        }
    }

    #[test]
    fn live_leases_keep_generations_until_the_last_terminal_closes() {
        let root = unique_dir("keep");
        let config = config_in(&root);
        let store = Store::open(config.plugins_state_dir());
        let (generation, dir) = store.prepare_generation("dev.example.git").unwrap();
        std::fs::write(dir.join("bin"), "one").unwrap();
        store.set_current("dev.example.git", &dir).unwrap();
        store.save_record(&record(generation)).unwrap();
        acquire(
            &config,
            "dev.example.git",
            generation,
            &dir,
            "dev.example.git.git-ui#7",
        )
        .unwrap();
        acquire(
            &config,
            "dev.example.git",
            generation,
            &dir,
            "dev.example.git.git-ui#8",
        )
        .unwrap();
        assert_eq!(
            live_generations(&config, "dev.example.git"),
            vec![generation]
        );

        // 卸载只删记录与指针，代际留在原处供运行中的终端使用。
        store.detach("dev.example.git").unwrap();
        assert!(store.record("dev.example.git").is_none());
        assert!(dir.exists());

        assert!(release(&config, "dev.example.git", "dev.example.git.git-ui#7").is_empty());
        assert!(dir.exists());
        assert_eq!(
            release(&config, "dev.example.git", "dev.example.git.git-ui#8"),
            vec![generation]
        );
        assert!(!dir.exists());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn two_generations_are_kept_side_by_side() {
        let root = unique_dir("generations");
        let config = config_in(&root);
        let store = Store::open(config.plugins_state_dir());
        let (first, dir) = store.prepare_generation("dev.example.git").unwrap();
        store.set_current("dev.example.git", &dir).unwrap();
        acquire(&config, "dev.example.git", first, &dir, "git-ui#1").unwrap();
        let (second, dir2) = store.prepare_generation("dev.example.git").unwrap();
        assert_eq!(second, first + 1);
        acquire(&config, "dev.example.git", second, &dir2, "git-ui#2").unwrap();
        assert_eq!(
            live_generations(&config, "dev.example.git"),
            vec![first, second]
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn dead_process_leases_are_cleaned_up() {
        let root = unique_dir("dead");
        let config = config_in(&root);
        let path = lease_path(&config, "dev.example.git", 999_999, 4);
        write(
            &path,
            &json!({
                "package": "dev.example.git",
                "pid": 999_999,
                "generation": 4,
                "root": "/nowhere",
                "terminals": ["t"],
            }),
        )
        .unwrap();
        assert!(live_generations(&config, "dev.example.git").is_empty());
        assert!(!path.exists());
        std::fs::remove_dir_all(root).ok();
    }
}
