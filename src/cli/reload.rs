//! reload 类命令的结果形状：`{status, diagnostics}`。
//!
//! 数据形状：`status` 只有 `applied` / `rejected`，`diagnostics` 每项是
//! `{"severity":"error"|"warning","message"}`。
//! 模块边界：诊断文本由调用方从配置或清单里取，渲染与「当前使用的是旧配置」
//! 提示只在这里。
//! 不变量：`rejected` 时一定打印旧配置提示；调用方按 [`Report::exit`] 退出。

use hc_tui::config::{self, Config};
use hc_tui::i18n::t;

/// 重载结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// 新配置、新代际或新模块已经生效。
    Applied,
    /// 拒绝应用，运行中的仍然是旧的那一份。
    Rejected,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Applied => "applied",
            Status::Rejected => "rejected",
        }
    }
}

/// 诊断级别。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
        }
    }
}

/// 一条诊断。
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
}

impl Diagnostic {
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            message: message.into(),
        }
    }

    pub fn warning(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            message: message.into(),
        }
    }
}

/// 一次重载的结果。
#[derive(Debug, Clone)]
pub struct Report {
    pub status: Status,
    pub diagnostics: Vec<Diagnostic>,
}

impl Report {
    /// 已经生效的一侧：清单提示与服务问题都算诊断。
    pub fn applied(diagnostics: Vec<Diagnostic>) -> Self {
        Self {
            status: Status::Applied,
            diagnostics,
        }
    }

    /// 从刚解析的配置取诊断：有错误就整次拒绝，警告只记录。
    pub fn from_config(config: &Config) -> Self {
        let path = &config.config_path;
        let mut diagnostics: Vec<Diagnostic> = config
            .errors
            .iter()
            .map(|error| Diagnostic::error(config::describe(path, error)))
            .collect();
        // 解析错误会同时进 warnings；已经在错误里的那条不重复算警告。
        let known: Vec<String> = diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.clone())
            .collect();
        let warnings: Vec<Diagnostic> = config
            .warnings
            .iter()
            .filter(|warning| !known.contains(warning))
            .cloned()
            .map(Diagnostic::warning)
            .collect();
        diagnostics.extend(warnings);
        let status = if config.errors.is_empty() {
            Status::Applied
        } else {
            Status::Rejected
        };
        Self {
            status,
            diagnostics,
        }
    }

    pub fn is_rejected(&self) -> bool {
        self.status == Status::Rejected
    }

    /// 进程退出码：拒绝算操作失败，其余成功。
    pub fn exit(&self) -> hc_tui::error::Exit {
        if self.is_rejected() {
            hc_tui::error::Exit::Failure
        } else {
            hc_tui::error::Exit::Ok
        }
    }

    /// 打印结果；`json` 走契约形状，人类形态先打 `status`，拒绝时补旧配置提示。
    pub fn emit(&self, json: bool) {
        if json {
            let diagnostics: Vec<serde_json::Value> = self
                .diagnostics
                .iter()
                .map(|diagnostic| {
                    serde_json::json!({
                        "severity": diagnostic.severity.as_str(),
                        "message": diagnostic.message,
                    })
                })
                .collect();
            println!(
                "{}",
                serde_json::json!({"status": self.status.as_str(), "diagnostics": diagnostics})
            );
            return;
        }
        println!("status {}", self.status.as_str());
        for diagnostic in &self.diagnostics {
            println!("  {}: {}", diagnostic.severity.as_str(), diagnostic.message);
        }
        if self.is_rejected() {
            println!(
                "{}",
                t("the old configuration is still in use; fix the problems above and rerun")
            );
        }
    }
}
