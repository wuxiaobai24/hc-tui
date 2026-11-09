//! CLI 失败契约：稳定错误码、退出码与两种错误形状。
//!
//! 数据形状：失败统一成 [`CliError`]（码 + 可读文案）。`--json` 时渲染成
//! `{"error":{"code","message"},"id":"cli:<动词>[:<子动词>]"}` 打到 stderr，
//! 人类形态在文案之后追加末行 `(code: <码>)`。
//! 模块边界：抛错留在各命令里，渲染与退出码只在这里决定，`src/main.rs` 是唯一出口。
//! 不变量：[`ErrorCode`] 是码集的唯一来源，`docs/cli-errors.md` 的表格由本文件的
//! 单测逐行对拍（多一个、少一个都红）；文案必须回显用户输入的 token。

use std::fmt;

/// 进程退出码：全仓只认这四个值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum Exit {
    /// 成功。
    Ok = 0,
    /// 操作失败，错误对象已经输出。
    Failure = 1,
    /// 用法错误：参数或子命令不认识。
    Usage = 2,
    /// core 不可用：没有会话在跑，或连不上、协议对不上。
    NoCore = 3,
}

impl Exit {
    /// 传给 `std::process::exit` 的整数。
    pub fn code(self) -> i32 {
        self as i32
    }
}

/// 稳定错误码：小写 snake_case，`<名词>_<状态>`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ErrorCode {
    /// 参数或子命令不认识。
    Usage,
    /// 找不到包（含插件与模块）。
    PackageNotFound,
    /// 找不到 pane。
    PaneNotFound,
    /// 找不到终端。
    TerminalNotFound,
    /// 找不到机器。
    MachineNotFound,
    /// 找不到文件或安装来源。
    FileNotFound,
    /// 目标文件已经存在（`config init` 不带 `--force`）。
    FileExists,
    /// 找不到包声明的服务。
    ServiceNotFound,
    /// 包清单非法。
    ManifestInvalid,
    /// 配置非法。
    ConfigInvalid,
    /// 本会话没有 core，或连不上。
    CoreUnavailable,
    /// 包清单声明的 host api 与当前宿主不兼容。
    HostApiTooOld,
    /// core 与客户端协议不兼容。
    ProtocolIncompatible,
    /// 权限拒绝。
    PermissionDenied,
    /// 操作超时。
    Timeout,
    /// 这个动作需要运行中的 TUI 窗口。
    WindowRequired,
    /// 共享布局文档缺失或写入冲突。
    LayoutInvalid,
    /// 安装被用户取消。
    InstallCancelled,
    /// 更新检查或更新失败。
    UpdateFailed,
    /// 未归类的内部错误（含外部工具失败）。
    Internal,
}

impl ErrorCode {
    /// 全部错误码，顺序与 `docs/cli-errors.md` 的码表一致。
    pub const ALL: &'static [ErrorCode] = &[
        ErrorCode::Usage,
        ErrorCode::PackageNotFound,
        ErrorCode::PaneNotFound,
        ErrorCode::TerminalNotFound,
        ErrorCode::MachineNotFound,
        ErrorCode::FileNotFound,
        ErrorCode::FileExists,
        ErrorCode::ServiceNotFound,
        ErrorCode::ManifestInvalid,
        ErrorCode::ConfigInvalid,
        ErrorCode::CoreUnavailable,
        ErrorCode::HostApiTooOld,
        ErrorCode::ProtocolIncompatible,
        ErrorCode::PermissionDenied,
        ErrorCode::Timeout,
        ErrorCode::WindowRequired,
        ErrorCode::LayoutInvalid,
        ErrorCode::InstallCancelled,
        ErrorCode::UpdateFailed,
        ErrorCode::Internal,
    ];

    /// 词表里的 snake_case 串；脚本依赖它，不许改名。
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::Usage => "usage",
            ErrorCode::PackageNotFound => "package_not_found",
            ErrorCode::PaneNotFound => "pane_not_found",
            ErrorCode::TerminalNotFound => "terminal_not_found",
            ErrorCode::MachineNotFound => "machine_not_found",
            ErrorCode::FileNotFound => "file_not_found",
            ErrorCode::FileExists => "file_exists",
            ErrorCode::ServiceNotFound => "service_not_found",
            ErrorCode::ManifestInvalid => "manifest_invalid",
            ErrorCode::ConfigInvalid => "config_invalid",
            ErrorCode::CoreUnavailable => "core_unavailable",
            ErrorCode::HostApiTooOld => "host_api_too_old",
            ErrorCode::ProtocolIncompatible => "protocol_incompatible",
            ErrorCode::PermissionDenied => "permission_denied",
            ErrorCode::Timeout => "timeout",
            ErrorCode::WindowRequired => "window_required",
            ErrorCode::LayoutInvalid => "layout_invalid",
            ErrorCode::InstallCancelled => "install_cancelled",
            ErrorCode::UpdateFailed => "update_failed",
            ErrorCode::Internal => "internal_error",
        }
    }

    /// 这个码对应的进程退出码。
    pub fn exit(self) -> Exit {
        match self {
            ErrorCode::Usage => Exit::Usage,
            ErrorCode::CoreUnavailable | ErrorCode::ProtocolIncompatible => Exit::NoCore,
            _ => Exit::Failure,
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// 一个已经定好码的失败：文案就是人类形态的正文。
#[derive(Debug)]
pub struct CliError {
    pub code: ErrorCode,
    pub message: String,
}

impl CliError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CliError {}

/// 命令已经自己打印了结果（例如 reload 的 `{status,diagnostics}`），只需要一个退出码。
#[derive(Debug)]
pub struct Reported(pub Exit);

impl fmt::Display for Reported {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("")
    }
}

impl std::error::Error for Reported {}

/// 已经打印过结果，只带退出码上报。
pub fn reported(exit: Exit) -> anyhow::Error {
    anyhow::Error::new(Reported(exit))
}

/// 带码的失败；文案自己给。
pub fn fail(code: ErrorCode, message: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(CliError::new(code, message))
}

/// 用法错误：退出码 2。
pub fn usage(message: impl Into<String>) -> anyhow::Error {
    fail(ErrorCode::Usage, message)
}

/// 给已有错误换一个码；把整条原因链压进文案，码留在最外层。
pub fn coded(error: anyhow::Error, code: ErrorCode) -> anyhow::Error {
    fail(code, format!("{error:#}"))
}

/// 清单类失败：host api 对不上单列一个码，其余按清单非法。
pub fn manifest(error: impl fmt::Display) -> anyhow::Error {
    let message = error.to_string();
    // 只有「host api 对不上」这一条单列码；别的清单错误都算 manifest_invalid。
    let code = if message.contains("host_api ") && message.contains("not supported") {
        ErrorCode::HostApiTooOld
    } else {
        ErrorCode::ManifestInvalid
    };
    fail(code, message)
}

/// 安装来源找不到：路径、git 地址或归档都不成立。
pub fn source(error: impl fmt::Display) -> anyhow::Error {
    fail(ErrorCode::FileNotFound, error.to_string())
}

/// 目标已经存在，且没带覆盖开关。
pub fn exists(error: impl fmt::Display) -> anyhow::Error {
    fail(ErrorCode::FileExists, error.to_string())
}

/// 按 id 找不到对象。
pub fn not_found(error: impl fmt::Display) -> anyhow::Error {
    fail(ErrorCode::PackageNotFound, error.to_string())
}

/// 控制 socket 的失败：应答解析不了算协议不兼容，其余按 core 不可用；
/// 权限问题单独成码（例如 socket 或目录不可读）。
pub fn control(error: anyhow::Error) -> anyhow::Error {
    if error.downcast_ref::<serde_json::Error>().is_some() {
        return coded(error, ErrorCode::ProtocolIncompatible);
    }
    if error
        .downcast_ref::<std::io::Error>()
        .is_some_and(|error| error.kind() == std::io::ErrorKind::PermissionDenied)
    {
        return coded(error, ErrorCode::PermissionDenied);
    }
    coded(error, ErrorCode::CoreUnavailable)
}

/// 无窗口动作的拒绝：`kind` 与文案都来自 core 侧的动作执行结果。
pub fn refusal(error: anyhow::Error) -> anyhow::Error {
    if let Some(refusal) = error.downcast_ref::<crate::app::winless::Refusal>() {
        return action_failure(refusal.kind, refusal.message.clone());
    }
    error
}

/// 控制协议应答里的失败：`error_kind` 是稳定信号，映射到词表。
pub fn action_failure(kind: &str, message: impl Into<String>) -> anyhow::Error {
    let message = message.into();
    let code = match kind {
        "no_core" => ErrorCode::CoreUnavailable,
        "needs_window" => ErrorCode::WindowRequired,
        "no_layout" | "layout_error" => ErrorCode::LayoutInvalid,
        "needs_input" => ErrorCode::Usage,
        // 动作失败的文案里带着对象（`unknown pane t9`），按同一个兜底表认码。
        _ => text_code(&message),
    };
    fail(code, message)
}

/// 从错误的类型与文案里认出码：显式码优先，其次 io 类型，最后是文案兜底。
pub fn classify(error: &anyhow::Error) -> ErrorCode {
    if error.downcast_ref::<Reported>().is_some() {
        return ErrorCode::Internal;
    }
    if let Some(cli) = error.downcast_ref::<CliError>() {
        return cli.code;
    }
    if let Some(io) = error.downcast_ref::<std::io::Error>() {
        return match io.kind() {
            std::io::ErrorKind::PermissionDenied => ErrorCode::PermissionDenied,
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => ErrorCode::Timeout,
            std::io::ErrorKind::ConnectionRefused
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::NotConnected => ErrorCode::CoreUnavailable,
            std::io::ErrorKind::NotFound => ErrorCode::FileNotFound,
            _ => ErrorCode::Internal,
        };
    }
    text_code(&format!("{error:#}"))
}

/// 缺少参数的文案：英文原文与中文译文都认，别的一律不算用法错误。
const USAGE_MARKERS: &[&str] = &[
    "required",
    "needs a value",
    "needs a number",
    "needs a text",
    "缺少",
    "需要一个值",
    "需要一个数字",
    "需要终端",
    "需要文本",
    "需要名称",
    "需要命令参数",
    "需要请求 id",
];

/// 文案兜底：只认几条稳定前缀，认不出就是内部错误。
pub fn text_code(text: &str) -> ErrorCode {
    let lower = text.to_ascii_lowercase();
    if lower.contains("unknown pane") || lower.contains("未知 pane") {
        ErrorCode::PaneNotFound
    } else if lower.contains("unknown terminal") || lower.contains("未知终端") {
        ErrorCode::TerminalNotFound
    } else if lower.contains("unknown machine")
        || lower.contains("未知机器")
        || lower.contains("machine_unknown")
    {
        ErrorCode::MachineNotFound
    } else if lower.contains("host_api") || lower.contains("host api") {
        ErrorCode::HostApiTooOld
    } else if lower.contains("找不到模块") || lower.contains("找不到包") {
        ErrorCode::PackageNotFound
    } else if lower.contains("manifest") || lower.contains("hctui.toml") {
        ErrorCode::ManifestInvalid
    } else if lower.contains("unsupported protocol") {
        ErrorCode::ProtocolIncompatible
    } else if USAGE_MARKERS.iter().any(|marker| lower.contains(marker)) {
        ErrorCode::Usage
    } else {
        ErrorCode::Internal
    }
}

/// `id`：`cli:` + 动词路径。动词路径只取命令名与子命令，参数不算。
pub fn command_id(args: &[String]) -> String {
    const TWO_LEVEL: &[&str] = &[
        "packages",
        "plugins",
        "mods",
        "config",
        "theme",
        "keys",
        "completion",
        "session-hook",
    ];
    let Some(first) = args.first().filter(|arg| !arg.starts_with('-')) else {
        return "cli:hc-tui".to_string();
    };
    let mut parts = vec![first.as_str()];
    if TWO_LEVEL.contains(&first.as_str()) {
        if let Some(second) = args.get(1).filter(|arg| !arg.starts_with('-')) {
            parts.push(second.as_str());
            if first == "plugins" && matches!(second.as_str(), "action" | "pane") {
                if let Some(third) = args.get(2).filter(|arg| !arg.starts_with('-')) {
                    parts.push(third.as_str());
                }
            }
        }
    }
    format!("cli:{}", parts.join(":"))
}

/// 这一行调用要的是机器可读的失败形状吗。
pub fn wants_json(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "--json")
}

/// 唯一的失败出口：按形状打印，返回进程退出码。
pub fn report(error: &anyhow::Error, args: &[String]) -> i32 {
    if let Some(reported) = error.downcast_ref::<Reported>() {
        return reported.0.code();
    }
    let code = classify(error);
    let message = format!("{error:#}");
    if wants_json(args) {
        let body = serde_json::json!({
            "error": {"code": code.as_str(), "message": message},
            "id": command_id(args),
        });
        eprintln!("{body}");
    } else {
        eprintln!("{message}");
        eprintln!("(code: {})", code.as_str());
    }
    code.exit().code()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = include_str!("../docs/cli-errors.md");

    /// 码表里的行：`| `code` | 含义 | 退出码 |`。
    fn code_rows() -> Vec<Vec<&'static str>> {
        DOC.lines()
            .filter(|line| line.starts_with("| `"))
            .map(|line| line.trim_matches('|').split('|').map(str::trim).collect())
            .collect()
    }

    fn documented() -> Vec<&'static str> {
        code_rows()
            .iter()
            .map(|cells| cells[0].trim_matches('`'))
            .collect()
    }

    #[test]
    fn the_doc_table_lists_exactly_the_code_set() {
        let expected: Vec<&str> = ErrorCode::ALL.iter().map(|code| code.as_str()).collect();
        assert_eq!(
            documented(),
            expected,
            "docs/cli-errors.md 的码表必须与 ErrorCode::ALL 逐行一致（多一个或漏一个都算漂移）"
        );
    }

    #[test]
    fn every_code_has_a_description_and_its_exit_code() {
        for cells in code_rows() {
            let name = cells[0].trim_matches('`');
            let known = ErrorCode::ALL
                .iter()
                .find(|code| code.as_str() == name)
                .unwrap_or_else(|| panic!("{name} 不在 ErrorCode::ALL 里"));
            assert!(
                cells.len() >= 3,
                "{name} 的行只有 {} 列，需要 码 / 含义 / 退出码",
                cells.len()
            );
            assert!(!cells[1].is_empty(), "{name} 缺一行说明");
            assert_eq!(
                cells[2],
                known.exit().code().to_string(),
                "{name} 的退出码与 ErrorCode::exit 不一致"
            );
        }
    }

    #[test]
    fn the_doc_explains_all_four_exit_codes() {
        for exit in [Exit::Ok, Exit::Failure, Exit::Usage, Exit::NoCore] {
            assert!(
                DOC.contains(&format!("| {} |", exit.code())),
                "docs/cli-errors.md 缺退出码 {} 的行",
                exit.code()
            );
        }
    }

    #[test]
    fn the_vocabulary_is_snake_case_and_unique() {
        let mut names: Vec<&str> = ErrorCode::ALL.iter().map(|code| code.as_str()).collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count, "ErrorCode::ALL 里有重复项");
        for name in ErrorCode::ALL.iter().map(|code| code.as_str()) {
            assert!(
                name.chars().all(|character| character.is_ascii_lowercase()
                    || character.is_ascii_digit()
                    || character == '_'),
                "{name} 不是小写 snake_case"
            );
        }
    }

    #[test]
    fn the_id_follows_the_verb_path() {
        let id =
            |args: &[&str]| command_id(&args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>());
        assert_eq!(id(&["packages", "install", "x"]), "cli:packages:install");
        assert_eq!(id(&["pane", "get"]), "cli:pane");
        assert_eq!(
            id(&["plugins", "action", "invoke", "x"]),
            "cli:plugins:action:invoke"
        );
        assert_eq!(id(&["do", "terminal.park"]), "cli:do");
        assert_eq!(id(&["--json", "list"]), "cli:hc-tui");
    }

    #[test]
    fn the_text_fallback_reads_both_languages() {
        assert_eq!(text_code("unknown pane t9"), ErrorCode::PaneNotFound);
        assert_eq!(text_code("未知 Pane t9"), ErrorCode::PaneNotFound);
        assert_eq!(
            text_code("unknown terminal local/t9"),
            ErrorCode::TerminalNotFound
        );
        assert_eq!(text_code("未知终端 local/t9"), ErrorCode::TerminalNotFound);
        assert_eq!(text_code("terminal required"), ErrorCode::Usage);
        assert_eq!(text_code("缺少终端"), ErrorCode::Usage);
        assert_eq!(text_code("everything is on fire"), ErrorCode::Internal);
    }

    #[test]
    fn a_coded_error_keeps_its_code_and_message() {
        let error = coded(
            anyhow::anyhow!("connect: refused"),
            ErrorCode::CoreUnavailable,
        );
        assert_eq!(classify(&error), ErrorCode::CoreUnavailable);
        assert_eq!(format!("{error:#}"), "connect: refused");
        let permission = std::io::Error::from(std::io::ErrorKind::PermissionDenied);
        assert_eq!(
            classify(&anyhow::Error::new(permission)),
            ErrorCode::PermissionDenied
        );
    }
}
