//! `[[services]]` 的纯规划部分：把一条服务声明翻译成 core 的 `terminal.run` 参数。
//!
//! 数据形状 = [`crate::plugin::registry::ServiceInfo`] → [`Plan`]；运行期台账
//! 由 `src/app/services.rs` 写进布局文档，这里不碰任何文档或连接。
//! 模块边界：清单解析在 [`super::manifest`]，注册表在 [`super::registry`]，
//! 执行体是 core 的 keep 终端。
//!
//! 不变量：同一个 `(package, service)` 只对应一个终端名 [`terminal_name`]，
//! core 对重名报 `name_taken`，这就是“重开窗口不产生第二个终端”的那道闸门；
//! 程序不存在时在规划期就报 [`MISSING_PROGRAM`]，不静默交给 core。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::json;
use serde_json::Value;

use crate::plugin::registry::ServiceInfo;

/// 目标机器上找不到程序。
pub const MISSING_PROGRAM: &str = "missing_program";
/// 声明与文件系统不一致（cwd 缺失等）。
pub const UNAVAILABLE: &str = "unavailable";

/// 终端活着，程序还没起来（core 的 `spawn_pending`）。
pub const STARTING: &str = "starting";
/// 终端活着，程序在跑。
pub const RUNNING: &str = "running";
/// 程序自己结束了（正常退出或被杀），不自动重启。
pub const EXITED: &str = "exited";
/// 起不来：`argv` 程序缺失，或启动后 [`FAILURE_WINDOW_MS`] 内非 0 退出。
pub const FAILED: &str = "failed";
/// 没有终端记录：未启动、包已停用，或 core 不在。
pub const STOPPED: &str = "stopped";

/// 启动后这么快就以非 0 结束，说明它根本没起来。
pub const FAILURE_WINDOW_MS: u64 = 1000;

/// 规划或探测失败。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanError {
    /// [`MISSING_PROGRAM`] 或 [`UNAVAILABLE`]。
    pub kind: &'static str,
    pub message: String,
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.kind, self.message)
    }
}

impl std::error::Error for PlanError {}

/// 一条服务物化成终端所需的全部参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// `<package>.<service>`。
    pub name: String,
    /// 包 ID。
    pub package: String,
    /// 服务 ID。
    pub service: String,
    /// 终端标题。
    pub title: String,
    /// core 的终端名；同一 `(package, service)` 恒定。
    pub terminal_name: String,
    /// `terminal.run` 的 `argv`。
    pub argv: Vec<String>,
    /// `terminal.run` 的 `cwd`。
    pub cwd: Option<String>,
    /// `terminal.run` 的 `env`。
    pub env: BTreeMap<String, String>,
    /// 日志文件的绝对路径。
    pub log: Option<PathBuf>,
}

/// `packages list` 里一条服务的运行状态；唯一来源是 core 的 `terminal.list`。
///
/// 没有本地 PID 探测：`pid` 只是 core 记录里的字段，用于展示。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ServiceStatus {
    /// `<package>.<service>`。
    pub id: String,
    /// [`STARTING`] / [`RUNNING`] / [`EXITED`] / [`FAILED`] / [`STOPPED`]。
    pub state: &'static str,
    /// core 的终端地址；没有终端记录时为 `null`。
    pub terminal: Option<String>,
    /// 活终端的进程号。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u64>,
    /// 失败或结束的原因。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl ServiceStatus {
    /// 已停用包的服务：不跑，也不去探测程序。
    pub fn stopped(info: &ServiceInfo) -> Self {
        Self {
            id: info.name.clone(),
            state: STOPPED,
            terminal: None,
            pid: None,
            reason: None,
        }
    }
}

/// 当前时间的毫秒；台账的 `started_ms`。
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// 包数据目录：日志与运行期文件的落点。
pub fn data_dir(state_root: &Path, package: &str) -> PathBuf {
    state_root.join("data").join(package)
}

/// core 的终端名：`(package, service)` 的稳定短哈希。
///
/// core 要求 `[a-z0-9-_]{1,32}` 且活终端之间唯一，`name_taken` 就是幂等闸门。
pub fn terminal_name(package: &str, service: &str) -> String {
    use sha2::Digest;
    let digest = format!(
        "{:x}",
        sha2::Sha256::digest(format!("{package}.{service}").as_bytes())
    );
    format!("hcsvc-{}", &digest[..16])
}

/// 服务的标签：`terminal.list` 反查用。
pub fn labels(package: &str, service: &str) -> Value {
    json!({"hctui.service": format!("{package}.{service}")})
}

/// 程序在 PATH 里的绝对路径；找不到时为 `None`。
fn which(program: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

/// 解析 `argv[0]`：包内相对程序按代际目录展开，PATH 程序要求真实存在。
pub fn resolve_program(program: &str, root: &Path) -> Result<PathBuf, PlanError> {
    if let Some(relative) = program.strip_prefix("./") {
        let path = root.join(relative);
        if path.is_file() {
            return Ok(path);
        }
        return Err(PlanError {
            kind: MISSING_PROGRAM,
            message: format!("`{program}` is missing at {}", path.display()),
        });
    }
    if program.contains('/') {
        let path = PathBuf::from(program);
        if path.is_file() {
            return Ok(path);
        }
        return Err(PlanError {
            kind: MISSING_PROGRAM,
            message: format!("`{program}` is missing"),
        });
    }
    which(program).ok_or_else(|| PlanError {
        kind: MISSING_PROGRAM,
        message: format!("`{program}` is not on PATH"),
    })
}

/// 把一条声明规划成 `terminal.run` 参数。
pub fn plan(info: &ServiceInfo, data_root: &Path) -> Result<Plan, PlanError> {
    let argv: Vec<&str> = info.argv.iter().map(String::as_str).collect();
    let Some(first) = argv.first() else {
        return Err(PlanError {
            kind: UNAVAILABLE,
            message: "service argv is empty".to_string(),
        });
    };
    let program = resolve_program(first, &info.root)?;
    let mut argv: Vec<String> = argv.iter().map(|value| value.to_string()).collect();
    argv[0] = program.display().to_string();
    let cwd = match &info.cwd {
        Some(cwd) => {
            let path = if cwd == "." {
                info.root.clone()
            } else {
                info.root.join(cwd)
            };
            if !path.is_dir() {
                return Err(PlanError {
                    kind: UNAVAILABLE,
                    message: format!("service cwd {} is missing", path.display()),
                });
            }
            Some(path.display().to_string())
        }
        None => Some(info.root.display().to_string()),
    };
    let log = log_path(info, data_root);
    if let Some(path) = &log {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| PlanError {
                kind: UNAVAILABLE,
                message: format!("cannot create {}: {error}", parent.display()),
            })?;
        }
        // 日志是服务的 stdout/stderr：用 sh 打开追加，不引入额外进程。
        let target = path.display().to_string();
        argv = vec![
            "sh".to_string(),
            "-c".to_string(),
            format!(
                "exec \"$0\" \"$@\" >> '{}' 2>&1",
                target.replace('\'', "'\\''")
            ),
        ]
        .into_iter()
        .chain(argv)
        .collect();
    }
    Ok(Plan {
        name: info.name.clone(),
        package: info.package.clone(),
        service: info.service.clone(),
        title: info.title.clone(),
        terminal_name: terminal_name(&info.package, &info.service),
        argv,
        cwd,
        env: info.env.clone(),
        log,
    })
}

/// `terminal.run` 的参数；`keep` 恒为真，服务活过窗口。
pub fn run_params(plan: &Plan) -> Value {
    let mut params = json!({
        "argv": plan.argv,
        "keep": true,
        "name": plan.terminal_name,
        "labels": labels(&plan.package, &plan.service),
    });
    if let Some(cwd) = &plan.cwd {
        params["cwd"] = json!(cwd);
    }
    if !plan.env.is_empty() {
        params["env"] = json!(plan.env);
    }
    params
}

/// 规划期能发现的缺失程序；`packages list` 与物化共用同一条判定。
pub fn missing_program(info: &ServiceInfo) -> Option<PlanError> {
    let first = info.argv.first()?;
    resolve_program(first, &info.root).err()
}

/// 服务的日志绝对路径；没有声明 `log` 时为 `None`。
pub fn log_path(info: &ServiceInfo, state_root: &Path) -> Option<PathBuf> {
    info.log
        .as_ref()
        .map(|log| data_dir(state_root, &info.package).join(log))
}

/// 日志文件的最后 `lines` 行。
pub fn log_tail(path: &Path, lines: usize) -> std::io::Result<String> {
    let text = std::fs::read_to_string(path)?;
    let all: Vec<&str> = text.lines().collect();
    let start = all.len().saturating_sub(lines);
    Ok(all[start..].join("\n"))
}

/// 一条服务的状态：core 的终端记录优先，没有记录才看声明的程序在不在。
pub fn status(info: &ServiceInfo, terminals: &[Value]) -> ServiceStatus {
    let name = terminal_name(&info.package, &info.service);
    let matches = |terminal: &&Value| {
        terminal.get("name").and_then(Value::as_str) == Some(name.as_str())
            || terminal
                .pointer("/labels/hctui.service")
                .and_then(Value::as_str)
                == Some(info.name.as_str())
    };
    let finished =
        |terminal: &&Value| terminal.get("state").and_then(Value::as_str) == Some("finished");
    let newest = |wanted_finished: bool| {
        terminals
            .iter()
            .filter(&matches)
            .filter(|terminal| finished(terminal) == wanted_finished)
            .max_by_key(|terminal| {
                terminal
                    .get("created_ms")
                    .and_then(Value::as_u64)
                    .unwrap_or(0)
            })
    };
    if let Some(record) = newest(false) {
        let pending = record.get("reason").and_then(Value::as_str) == Some("spawn_pending");
        let pid = record.get("pid").and_then(Value::as_u64);
        return ServiceStatus {
            id: info.name.clone(),
            state: if pending || pid.is_none() {
                STARTING
            } else {
                RUNNING
            },
            terminal: record
                .get("address")
                .and_then(Value::as_str)
                .map(str::to_string),
            pid,
            reason: pending.then(|| "spawn pending".to_string()),
        };
    }
    if let Some(record) = newest(true) {
        return ServiceStatus {
            id: info.name.clone(),
            state: if failed_at_once(record) {
                FAILED
            } else {
                EXITED
            },
            terminal: record
                .get("address")
                .and_then(Value::as_str)
                .map(str::to_string),
            pid: None,
            reason: Some(finish_reason(record)),
        };
    }
    let failure = if info.argv.is_empty() {
        Some(PlanError {
            kind: UNAVAILABLE,
            message: "service argv is empty".to_string(),
        })
    } else {
        missing_program(info)
    };
    match failure {
        Some(error) => ServiceStatus {
            id: info.name.clone(),
            state: FAILED,
            terminal: None,
            pid: None,
            reason: Some(error.to_string()),
        },
        None => ServiceStatus::stopped(info),
    }
}

/// 启动后 [`FAILURE_WINDOW_MS`] 内以非 0 退出：这条服务没起来。被杀不算失败。
fn failed_at_once(record: &Value) -> bool {
    if record.get("reason").and_then(Value::as_str) == Some("killed") {
        return false;
    }
    let code = record.get("exit_code").and_then(Value::as_i64);
    if code.is_none_or(|code| code == 0) {
        return false;
    }
    let created = record.get("created_ms").and_then(Value::as_u64);
    let finished = record.get("finished_ms").and_then(Value::as_u64);
    match (created, finished) {
        (Some(created), Some(finished)) => finished.saturating_sub(created) <= FAILURE_WINDOW_MS,
        _ => false,
    }
}

/// core 的结束信息压成一行：`exit 3 after 0.2s`、`killed`、`signal 9`。
fn finish_reason(record: &Value) -> String {
    let core = record
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or("exited");
    let mut text = if core == "killed" {
        core.to_string()
    } else if let Some(code) = record.get("exit_code").and_then(Value::as_i64) {
        format!("exit {code}")
    } else if let Some(signal) = record.get("signal").and_then(Value::as_i64) {
        format!("signal {signal}")
    } else {
        core.to_string()
    };
    let created = record.get("created_ms").and_then(Value::as_u64);
    let finished = record.get("finished_ms").and_then(Value::as_u64);
    if let (Some(created), Some(finished)) = (created, finished) {
        text.push_str(&format!(
            " after {:.1}s",
            finished.saturating_sub(created) as f64 / 1000.0
        ));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Source;

    fn info(root: PathBuf, argv: &[&str], log: Option<&str>) -> ServiceInfo {
        ServiceInfo {
            name: "dev.example.svc.watch".into(),
            package: "dev.example.svc".into(),
            service: "watch".into(),
            title: "Watcher".into(),
            argv: argv.iter().map(|value| value.to_string()).collect(),
            cwd: None,
            env: [("RUST_LOG".to_string(), "info".to_string())]
                .into_iter()
                .collect(),
            on: "enable".into(),
            log: log.map(str::to_string),
            root,
            generation: 1,
            source: Source::Local,
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        let dir = std::env::temp_dir().join(format!("hctui-service-{tag}-{unique}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_terminal_name_is_stable_and_within_the_core_rule() {
        let name = terminal_name("dev.example.svc", "watch");
        assert_eq!(name, terminal_name("dev.example.svc", "watch"));
        assert_ne!(name, terminal_name("dev.example.svc", "other"));
        assert!(name.len() <= 32, "{name}");
        assert!(name
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-'));
    }

    #[test]
    fn a_missing_program_is_reported_at_plan_time() {
        let dir = temp_dir("missing");
        let spec = info(dir.clone(), &["definitely-missing-hctui-svc"], None);
        let error = plan(&spec, &dir).unwrap_err();
        assert_eq!(error.kind, MISSING_PROGRAM);
        assert!(error.message.contains("definitely-missing-hctui-svc"));
        assert_eq!(missing_program(&spec).unwrap().kind, MISSING_PROGRAM);

        let spec = info(dir.clone(), &["./bin/watch.sh"], None);
        assert_eq!(missing_program(&spec).unwrap().kind, MISSING_PROGRAM);
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("bin/watch.sh"), "#!/bin/sh\n").unwrap();
        let plan = plan(&spec, &dir).unwrap();
        assert_eq!(plan.argv[0], dir.join("bin/watch.sh").display().to_string());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_service_with_a_log_runs_through_a_redirect() {
        let dir = temp_dir("log");
        std::fs::write(dir.join("watch.sh"), "#!/bin/sh\n").unwrap();
        let spec = info(
            dir.clone(),
            &["./watch.sh", "--once"],
            Some("services/watch.log"),
        );
        let plan = plan(&spec, &dir).unwrap();
        assert_eq!(plan.argv[0], "sh");
        assert_eq!(plan.argv[1], "-c");
        assert!(plan.argv[2].contains(">> '"), "{}", plan.argv[2]);
        assert!(plan.argv[2].ends_with("2>&1"), "{}", plan.argv[2]);
        assert!(plan.argv[3].ends_with("watch.sh"));
        assert_eq!(plan.argv[4], "--once");
        let log = plan.log.clone().unwrap();
        assert!(log.ends_with("data/dev.example.svc/services/watch.log"));
        assert!(log.parent().unwrap().is_dir());

        let params = run_params(&plan);
        assert_eq!(params["keep"], true);
        assert_eq!(params["name"], plan.terminal_name);
        assert_eq!(params["labels"]["hctui.service"], "dev.example.svc.watch");
        assert_eq!(params["env"]["RUST_LOG"], "info");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_missing_cwd_is_unavailable() {
        let dir = temp_dir("cwd");
        std::fs::write(dir.join("watch.sh"), "#!/bin/sh\n").unwrap();
        let mut spec = info(dir.clone(), &["./watch.sh"], None);
        spec.cwd = Some("gone".into());
        let error = plan(&spec, &dir).unwrap_err();
        assert_eq!(error.kind, UNAVAILABLE);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn service_states_follow_the_core_terminal_records() {
        use serde_json::json;
        let dir = temp_dir("state");
        std::fs::write(dir.join("watch.sh"), "#!/bin/sh\n").unwrap();
        let spec = info(dir.clone(), &["./watch.sh"], Some("services/watch.log"));
        let label = || json!({"hctui.service": "dev.example.svc.watch"});

        let running = json!({
            "address": "t2",
            "name": terminal_name("dev.example.svc", "watch"),
            "state": "live", "pid": 4242, "created_ms": 1000,
            "labels": label(),
        });
        let status = super::status(&spec, &[running]);
        assert_eq!(status.state, RUNNING);
        assert_eq!(status.terminal.as_deref(), Some("t2"));
        assert_eq!(status.pid, Some(4242));
        assert_eq!(status.reason, None);

        // 启动不到 1 秒就非 0 退出：failed，带原因。
        let crash = json!({
            "address": "t3", "name": "hcsvc-crash", "state": "finished",
            "reason": "exited", "exit_code": 3,
            "created_ms": 2000, "finished_ms": 2200, "labels": label(),
        });
        let status = super::status(&spec, &[crash]);
        assert_eq!(status.state, FAILED);
        assert_eq!(status.terminal.as_deref(), Some("t3"));
        let reason = status.reason.unwrap();
        assert!(reason.contains("exit 3"), "{reason}");
        assert!(reason.contains("0.2s"), "{reason}");

        // 跑久了才退出：exited，不是 failed。
        let late = json!({
            "address": "t4", "state": "finished", "reason": "exited", "exit_code": 1,
            "created_ms": 1000, "finished_ms": 5000, "labels": label(),
        });
        assert_eq!(super::status(&spec, &[late]).state, EXITED);

        // 被 kill：exited，原因保留。
        let killed = json!({
            "address": "t5", "state": "finished", "reason": "killed",
            "created_ms": 1000, "finished_ms": 2000, "labels": label(),
        });
        let status = super::status(&spec, &[killed]);
        assert_eq!(status.state, EXITED);
        assert_eq!(status.reason.as_deref(), Some("killed after 1.0s"));

        // 没有记录：stopped。
        assert_eq!(super::status(&spec, &[]).state, STOPPED);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_missing_program_is_a_failed_service_without_a_terminal() {
        let dir = temp_dir("failed-state");
        let spec = info(dir.clone(), &["definitely-missing-hctui-svc"], None);
        let status = super::status(&spec, &[]);
        assert_eq!(status.state, FAILED);
        assert_eq!(status.terminal, None);
        assert!(status.reason.unwrap().contains("not on PATH"));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn the_log_tail_reads_only_the_last_lines() {
        let dir = temp_dir("tail");
        let path = dir.join("watch.log");
        std::fs::write(&path, "1\n2\n3\n").unwrap();
        assert_eq!(log_tail(&path, 2).unwrap(), "2\n3");
        assert_eq!(log_tail(&path, 9).unwrap(), "1\n2\n3");
        std::fs::remove_dir_all(dir).ok();
    }
}
