//! 插件终端入口的规划与检查：参数映射、包内相对程序规则与目标机器程序探测。
//!
//! 入口只接受两种程序：目标机器 PATH 中的程序，以及 `./` 开头、相对代际目录
//! 的程序。后者只能在安装所在的客户端主机启动；目标机器不同时返回
//! `unsupported`，不换成本机执行。`requires_programs` 与 PATH 中的 `argv[0]`
//! 一律交给实际执行位置（core）检查，缺失时报 `missing_program` 并列出缺失项；
//! TUI 的环境变量不参与最终判决。

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::{json, Value};

use crate::plugin::manifest::PLACEMENTS;
use crate::plugin::registry::TerminalInfo;

/// 包内相对程序在远端请求时的错误码。
pub const UNSUPPORTED: &str = "unsupported";
/// 目标机器上找不到程序。
pub const MISSING_PROGRAM: &str = "missing_program";
/// 机器不可达或被移除。
pub const OFFLINE: &str = "offline";
/// 触发对象已不存在。
pub const TARGET_GONE: &str = "target_gone";
/// 包已安装但被停用。
pub const DISABLED: &str = "disabled";
/// 其余无法立即创建的情况（布局没有空间、客户端没有视图等）。
pub const UNAVAILABLE: &str = "unavailable";

/// 一次入口请求里可由调用方覆盖的参数。
#[derive(Debug, Clone, Default)]
pub struct Request {
    pub cwd: Option<String>,
    pub machine: Option<String>,
    pub placement: Option<String>,
}

/// 目标选择器：调用方给出的 Pane、终端地址或 Space。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TargetSelector {
    /// Pane 编号或终端地址；解析失败不回退当前焦点。
    pub pane_or_terminal: Option<String>,
    /// 稳定的 Space 身份（`sp<id>` 或共享文档里的 origin）。
    pub space: Option<String>,
    /// 目标机器；`local` 与空值都表示本机。
    pub machine: Option<String>,
    /// 工作目录。
    pub cwd: Option<String>,
}

impl TargetSelector {
    /// 是否指定了任何目标。
    pub fn is_empty(&self) -> bool {
        self.pane_or_terminal.is_none()
            && self.space.is_none()
            && self.machine.is_none()
            && self.cwd.is_none()
    }
}

/// 解析后的不可变执行目标；上下文、环境与放置都由它生成。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionTarget {
    /// 稳定的 Space 身份。
    pub space: String,
    /// Space 名称。
    pub space_name: String,
    /// 目标 Space 在本次运行里的下标，只用于布局；本实例看不见该 Space 时为 `None`。
    pub index: Option<usize>,
    /// 稳定的 Tab 身份。
    pub tab: String,
    /// 机器；`None` 表示本机。
    pub machine: Option<String>,
    /// 工作目录。
    pub cwd: Option<String>,
    /// 目标 Pane（本实例的视图句柄）。
    pub pane: Option<u32>,
    /// 目标终端地址。
    pub terminal: Option<String>,
}

/// 规划完成、等待放置的终端入口。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedTerminal {
    /// `terminal.run` 与布局参数。
    pub plan: Plan,
    /// 冻结下来的执行目标。
    pub target: ExecutionTarget,
}

/// 按冻结的目标规划一次入口。
pub fn prepare(
    info: &TerminalInfo,
    target: ExecutionTarget,
    placement: Option<String>,
) -> Result<PreparedTerminal, PlanError> {
    let plan = plan(
        info,
        &Request {
            cwd: target.cwd.clone(),
            machine: target.machine.clone(),
            placement,
        },
    )?;
    Ok(PreparedTerminal { plan, target })
}

/// 规划结果：直接映射到 `terminal.run` 与实际布局操作。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// `<package>.<terminal>`。
    pub name: String,
    pub package: String,
    pub terminal: String,
    pub generation: u64,
    pub root: PathBuf,
    pub title: String,
    pub placement: String,
    /// `terminal.run` 的 `on`；`None` 表示本机。
    pub machine: Option<String>,
    pub argv: Vec<String>,
    pub cwd: Option<String>,
    /// 需要在目标机器上存在的程序。
    pub programs: Vec<String>,
}

/// 带稳定错误码的规划或探测失败。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanError {
    /// `unsupported` / `missing_program` / `offline` / `target_gone` / `unavailable`。
    pub kind: &'static str,
    pub message: String,
}

impl PlanError {
    pub fn new(kind: &'static str, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.kind, self.message)
    }
}

impl std::error::Error for PlanError {}

/// 规划一次终端入口。
pub fn plan(info: &TerminalInfo, request: &Request) -> Result<Plan, PlanError> {
    let placement = request
        .placement
        .as_deref()
        .unwrap_or(&info.placement)
        .to_string();
    if !PLACEMENTS.contains(&placement.as_str()) {
        return Err(PlanError::new(
            UNAVAILABLE,
            format!(
                "placement must be {}, not `{placement}`",
                PLACEMENTS.join(", ")
            ),
        ));
    }
    let machine = request
        .machine
        .as_deref()
        .map(str::trim)
        .filter(|machine| !machine.is_empty() && *machine != "local")
        .map(str::to_string);
    let mut argv = info.argv.clone();
    let first = argv.first().cloned().unwrap_or_default();
    if first.is_empty() {
        return Err(PlanError::new(UNAVAILABLE, "terminal argv is empty"));
    }
    let mut programs: Vec<String> = info.requires_programs.clone();
    if let Some(relative) = first.strip_prefix("./") {
        if let Some(machine) = &machine {
            return Err(PlanError::new(
                UNSUPPORTED,
                format!(
                    "`{first}` runs only on the client that installed the plugin; machine `{machine}` cannot run package files"
                ),
            ));
        }
        let path = info.root.join(relative);
        if !path.is_file() {
            return Err(PlanError::new(
                MISSING_PROGRAM,
                format!("`{first}` is missing at {}", path.display()),
            ));
        }
        argv[0] = path.display().to_string();
    } else if !first.contains('/') && !programs.iter().any(|program| program == &first) {
        // PATH 中的程序同样在目标机器上检查，避免 core 只给出一个笼统的启动失败。
        programs.push(first.clone());
    }
    Ok(Plan {
        name: info.name.clone(),
        package: info.package.clone(),
        terminal: info.terminal.clone(),
        generation: info.generation,
        root: info.root.clone(),
        title: info.title.clone(),
        placement,
        machine,
        argv,
        cwd: request.cwd.clone().filter(|cwd| !cwd.is_empty()),
        programs,
    })
}

/// 目标机器上的探测脚本：逐行打印缺失的程序，全部存在时不输出。
pub fn probe_line(programs: &[String]) -> String {
    let mut line = String::from("for p in");
    for program in programs {
        line.push(' ');
        line.push_str(&shell_word(program));
    }
    line.push_str("; do command -v \"$p\" >/dev/null 2>&1 || printf '%s\\n' \"$p\"; done");
    line
}

/// 单引号包裹一个词，供 `sh -c` 使用。
fn shell_word(word: &str) -> String {
    format!("'{}'", word.replace('\'', "'\\''"))
}

/// 目标机器探测的 `machine.exec` 参数；`machine` 为 `None` 时由 core 在本机执行。
pub fn probe_params(programs: &[String], machine: Option<&str>, timeout: u64) -> Value {
    json!({
        "argv": ["sh", "-c", probe_line(programs)],
        "on": machine,
        "timeout": timeout,
    })
}

/// 从探测输出里取出确实缺失的程序；输出里没有的程序视为存在。
pub fn parse_missing(stdout: &str, programs: &[String]) -> Vec<String> {
    let reported: Vec<&str> = stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    programs
        .iter()
        .filter(|program| reported.contains(&program.as_str()))
        .cloned()
        .collect()
}

/// 缺失程序的错误。
pub fn missing_error(machine: Option<&str>, missing: &[String]) -> PlanError {
    let where_ = machine.unwrap_or("this client");
    PlanError::new(
        MISSING_PROGRAM,
        format!("missing program on {where_}: {}", missing.join(", ")),
    )
}

/// core 的 RPC 失败码映射到入口错误码；不认识的失败按不可用处理。
pub fn rpc_error(kind: &str, message: &str) -> PlanError {
    match kind {
        "machine_unknown" | "offline" | "unavailable" | "io" | "closed" | "timeout" => {
            PlanError::new(OFFLINE, message)
        }
        "target_gone" => PlanError::new(TARGET_GONE, message),
        _ => PlanError::new(UNAVAILABLE, message),
    }
}

/// 注入到终端程序的环境：动作变量加上 `HCTUI_CONTEXT` 与 `HCTUI_PLACEMENT`。
pub fn env(
    base: BTreeMap<String, String>,
    mut context: Value,
    placement: &str,
) -> BTreeMap<String, String> {
    let mut env = base;
    let encoded = serde_json::to_vec(&context).unwrap_or_else(|_| b"{}".to_vec());
    if encoded.len() > 32 * 1024 {
        if let Some(map) = context.as_object_mut() {
            map.remove("locale");
        }
        context = json!({"api_version": 1, "truncated": true});
    }
    env.insert("HCTUI_CONTEXT".into(), context.to_string());
    env.insert("HCTUI_PLACEMENT".into(), placement.to_string());
    env
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn entry(root: PathBuf, argv: Vec<&str>, requires: Vec<&str>) -> TerminalInfo {
        TerminalInfo {
            name: "dev.example.git.git-ui".into(),
            package: "dev.example.git".into(),
            terminal: "git-ui".into(),
            title: "Git UI".into(),
            placement: "tab".into(),
            argv: argv.into_iter().map(str::to_string).collect(),
            requires_programs: requires.into_iter().map(str::to_string).collect(),
            root,
            generation: 3,
            source: crate::store::Source::Local,
        }
    }

    #[test]
    fn remote_requests_never_run_package_relative_programs() {
        let dir = std::env::temp_dir().join("hctui-terminal-plan");
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("bin/git-ui.sh"), "#!/bin/sh\n").unwrap();
        let info = entry(dir.clone(), vec!["./bin/git-ui.sh"], vec![]);
        let local = plan(&info, &Request::default()).unwrap();
        assert_eq!(local.machine, None);
        assert!(local.argv[0].ends_with("bin/git-ui.sh"));
        assert!(Path::new(&local.argv[0]).is_absolute());

        let remote = plan(
            &info,
            &Request {
                machine: Some("peer".into()),
                ..Request::default()
            },
        );
        let error = remote.unwrap_err();
        assert_eq!(error.kind, UNSUPPORTED);
        assert!(error.message.contains("peer"), "{}", error.message);
        assert!(error.message.contains("./bin/git-ui.sh"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_package_file_and_path_programs_are_reported() {
        let dir = std::env::temp_dir().join("hctui-terminal-missing");
        std::fs::create_dir_all(&dir).unwrap();
        let info = entry(dir.clone(), vec!["./nope.sh"], vec![]);
        assert_eq!(
            plan(&info, &Request::default()).unwrap_err().kind,
            MISSING_PROGRAM
        );

        let info = entry(
            dir.clone(),
            vec!["lazygit"],
            vec!["lazygit", "definitely-missing-hctui-prog"],
        );
        let plan = plan(&info, &Request::default()).unwrap();
        // argv[0] 与 requires_programs 一起交给执行位置探测，TUI 不作判决。
        assert_eq!(
            plan.programs,
            vec!["lazygit", "definitely-missing-hctui-prog"]
        );
        let error = missing_error(None, &["definitely-missing-hctui-prog".to_string()]);
        assert_eq!(error.kind, MISSING_PROGRAM);
        assert!(error.message.contains("definitely-missing-hctui-prog"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn argv_programs_are_checked_and_unknown_placement_is_rejected() {
        let info = entry(std::env::temp_dir(), vec!["lazygit"], vec![]);
        let plan = plan(&info, &Request::default()).unwrap();
        assert_eq!(plan.programs, vec!["lazygit"]);
        let error = plan_with_placement(&info, "sideways").unwrap_err();
        assert_eq!(error.kind, UNAVAILABLE);
        assert!(error.message.contains("sideways"));
    }

    fn plan_with_placement(info: &TerminalInfo, placement: &str) -> Result<Plan, PlanError> {
        plan(
            info,
            &Request {
                placement: Some(placement.into()),
                ..Request::default()
            },
        )
    }

    #[test]
    fn probe_scripts_quote_programs_and_parse_only_reported_lines() {
        let programs = vec![
            "lazygit".to_string(),
            "we'ird prog".to_string(),
            "jq".to_string(),
        ];
        let line = probe_line(&programs);
        assert!(line.contains("'lazygit'"));
        assert!(line.contains("'we'\\''ird prog'"), "{line}");
        assert!(line.contains("command -v \"$p\""));
        let params = probe_params(&programs, Some("peer"), 20);
        assert_eq!(params["on"], json!("peer"));
        // 本机入口同样交给 core 探测，`on` 为 null。
        assert_eq!(probe_params(&programs, None, 20)["on"], Value::Null);
        assert_eq!(params["argv"][0], json!("sh"));
        let missing = parse_missing("we'ird prog\njq\n", &programs);
        assert_eq!(missing, vec!["we'ird prog".to_string(), "jq".to_string()]);
        assert!(parse_missing("", &programs).is_empty());
    }

    #[test]
    fn rpc_failures_map_to_stable_kinds() {
        assert_eq!(rpc_error("machine_unknown", "gone").kind, OFFLINE);
        assert_eq!(rpc_error("io", "closed").kind, OFFLINE);
        assert_eq!(rpc_error("target_gone", "gone").kind, TARGET_GONE);
        assert_eq!(rpc_error("malformed", "odd").kind, UNAVAILABLE);
    }

    #[test]
    fn terminal_env_carries_context_and_placement() {
        let base: BTreeMap<String, String> = [("HCTUI_PLUGIN_ID".to_string(), "p".to_string())]
            .into_iter()
            .collect();
        let env = env(base, json!({"api_version": 1}), "right");
        assert_eq!(env["HCTUI_PLUGIN_ID"], "p");
        assert_eq!(env["HCTUI_PLACEMENT"], "right");
        assert_eq!(env["HCTUI_CONTEXT"], "{\"api_version\":1}");
    }
}
