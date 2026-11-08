//! 面向用户的错误与崩溃诊断：三段式错误块、崩溃报告、模块调用上下文。
//!
//! 数据形状：`CallFrame{mod_id, method, seq}` 的全局环形缓冲，最多
//! [`CONTEXT_DEPTH`] 条，模块宿主的每次 `host.call` 压入一条。
//! 模块边界：本模块只依赖 `std` 与 [`crate::config`] 的路径推导；push 由
//! [`crate::mods::host`] 负责，安装钩子由 `main` 与测试负责。
//! 不变量：push 后长度截到上限；崩溃报告只读缓冲不清空，避免与 panic 竞态。

use std::collections::VecDeque;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

/// 提 issue 的地址。
pub const ISSUE_URL: &str = "https://github.com/wuxiaobai24/hc-tui/issues/new";

/// 模块调用上下文最多保留的层数。
pub const CONTEXT_DEPTH: usize = 6;

/// 一次宿主调用的定位信息。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallFrame {
    /// 模块 ID。
    pub mod_id: String,
    /// 宿主方法名（`state` / `run` / `exec` …）。
    pub method: String,
    /// 该表面的宿主调用序号，从 1 起算。
    pub seq: u64,
}

impl std::fmt::Display for CallFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "({}, {}, {})", self.mod_id, self.method, self.seq)
    }
}

fn context() -> &'static Mutex<VecDeque<CallFrame>> {
    static CONTEXT: OnceLock<Mutex<VecDeque<CallFrame>>> = OnceLock::new();
    CONTEXT.get_or_init(|| Mutex::new(VecDeque::new()))
}

/// 压入一条宿主调用；超过 [`CONTEXT_DEPTH`] 时丢掉最旧的一条。
pub fn record_call(mod_id: &str, method: &str, seq: u64) {
    let mut frames = context().lock().unwrap_or_else(|error| error.into_inner());
    frames.push_back(CallFrame {
        mod_id: mod_id.to_string(),
        method: method.to_string(),
        seq,
    });
    while frames.len() > CONTEXT_DEPTH {
        frames.pop_front();
    }
}

/// 最近的宿主调用，新到旧。
pub fn call_context() -> Vec<CallFrame> {
    let frames = context().lock().unwrap_or_else(|error| error.into_inner());
    frames.iter().rev().cloned().collect()
}

/// 清空上下文；测试用来隔离用例。
pub fn clear_call_context() {
    let mut frames = context().lock().unwrap_or_else(|error| error.into_inner());
    frames.clear();
}

/// 崩溃日志：`<state>/hc-tui/app.log`，与 `--config` 无关，按默认 XDG 状态目录。
pub fn log_path() -> PathBuf {
    crate::config::Config::default()
        .state_path
        .parent()
        .map(|parent| parent.join("app.log"))
        .unwrap_or_else(|| PathBuf::from("app.log"))
}

/// 复现命令：带 `RUST_BACKTRACE=1` 与本进程收到的参数。
pub fn repro_command() -> String {
    let mut command = String::from("RUST_BACKTRACE=1 hc-tui");
    for arg in std::env::args().skip(1) {
        command.push(' ');
        if arg.is_empty()
            || arg.contains(|character: char| {
                character.is_whitespace() || matches!(character, '"' | '\'' | '$' | '`' | '\\')
            })
        {
            command.push('\'');
            command.push_str(&arg.replace('\'', r"'\''"));
            command.push('\'');
        } else {
            command.push_str(&arg);
        }
    }
    command
}

/// 三段式错误块：可定位的事实、一句话解释、一条可粘贴的命令。
pub fn detail(fact: &str, situation: &str, fix: &str) -> String {
    format!("{fact}\n  你的情况：{situation}\nPossible fix: {fix}")
}

/// 三段式错误块，并列出全部查找过的位置。
pub fn not_found(fact: &str, situation: &str, searched: &[String], fix: &str) -> String {
    let mut text = format!("{fact}\n  你的情况：{situation}\n  已查找:\n");
    for path in searched {
        text.push_str("    - ");
        text.push_str(path);
        text.push('\n');
    }
    text.push_str("Possible fix: ");
    text.push_str(fix);
    text
}

/// 崩溃报告正文：三条固定出口 + 崩溃点 + 线程名 + 模块调用上下文。
pub fn crash_report(payload: &str, location: Option<&str>, thread: &str) -> String {
    let mut text = String::from("hc-tui 崩溃报告\n");
    text.push_str(&format!(
        "崩溃点: {}\n",
        location.unwrap_or("未知（编译器未提供位置）")
    ));
    text.push_str(&format!("线程: {thread}\n"));
    text.push_str(&format!("消息: {payload}\n"));
    let frames = call_context();
    if frames.is_empty() {
        text.push_str("模块调用上下文: 无（不是模块调用路径上的崩溃）\n");
    } else {
        text.push_str(&format!(
            "模块调用上下文（新→旧，最多 {CONTEXT_DEPTH} 层）:\n"
        ));
        for (index, frame) in frames.iter().enumerate() {
            text.push_str(&format!("  {}. {frame}\n", index + 1));
        }
    }
    text.push_str(&format!("日志文件: {}\n", log_path().display()));
    text.push_str(&format!("复现命令: {}\n", repro_command()));
    text.push_str(&format!("提 issue: {ISSUE_URL}\n"));
    text
}

/// 把一段文本追加到崩溃日志；返回写入的路径（失败时返回错误）。
pub fn append_log(text: &str) -> std::io::Result<PathBuf> {
    let path = log_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    file.write_all(text.as_bytes())?;
    Ok(path)
}

/// panic 钩子：打印崩溃报告、记进日志，再交给原钩子打标准 backtrace。
pub fn install() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|text| (*text).to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "非字符串 panic".to_string());
        let location = info
            .location()
            .map(|place| format!("{}:{}:{}", place.file(), place.line(), place.column()));
        let thread = std::thread::current()
            .name()
            .unwrap_or("<未命名线程>")
            .to_string();
        let text = crash_report(&payload, location.as_deref(), &thread);
        let mut stderr = std::io::stderr();
        let _ = stderr.write_all(text.as_bytes());
        let _ = stderr.flush();
        let _ = append_log(&text);
        previous(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_keeps_only_the_newest_six_calls() {
        clear_call_context();
        for seq in 1..=9 {
            record_call("demo", "state", seq);
        }
        let frames = call_context();
        assert_eq!(frames.len(), CONTEXT_DEPTH);
        assert_eq!(frames.first().map(|frame| frame.seq), Some(9));
        assert_eq!(
            frames.last().map(ToString::to_string).as_deref(),
            Some("(demo, state, 4)")
        );
        clear_call_context();
    }

    #[test]
    fn a_report_carries_the_three_exits_and_the_module_context() {
        clear_call_context();
        record_call("git-board", "state", 41);
        let text = crash_report("boom", Some("src/mods/render.rs:74:9"), "hc-engine");
        assert!(text.contains("崩溃点: src/mods/render.rs:74:9"), "{text}");
        assert!(text.contains("线程: hc-engine"), "{text}");
        assert!(text.contains("1. (git-board, state, 41)"), "{text}");
        assert!(text.contains("日志文件: "), "{text}");
        assert!(text.contains("复现命令: RUST_BACKTRACE=1 hc-tui"), "{text}");
        assert!(text.contains(ISSUE_URL), "{text}");
        clear_call_context();
    }

    #[test]
    fn a_not_found_block_lists_every_searched_location() {
        let text = not_found(
            "no mod at nope",
            "这个路径不存在，安装存储里也没有同名的包",
            &["nope".to_string(), "/tmp/mods/nope".to_string()],
            "hc-tui packages list",
        );
        assert!(text.contains("    - /tmp/mods/nope"), "{text}");
        assert!(
            text.contains("Possible fix: hc-tui packages list"),
            "{text}"
        );
    }
}
