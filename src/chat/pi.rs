//! pi 会话 JSONL 的解析：`~/.pi/agent/sessions/--<path>--/<时间>_<id>.jsonl`。
//!
//! 格式见 pi 的 `docs/session-format.md` 与 `docs/message-types.md`。条目按
//! `id` / `parentId` 组成一棵树；这里只沿当前叶子往上取一条链，被 `/tree`
//! 放弃的分支不会出现在 Chat 里。

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::i18n::t;

/// 一个可渲染的消息块。
#[derive(Debug, Clone)]
pub struct Block {
    pub kind: BlockKind,
    pub text: String,
    pub tool: Option<Tool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    /// 用户输入。
    User,
    /// agent 的正文。
    Assistant,
    /// agent 的思考内容。
    Thinking,
    /// 一次工具调用。
    Tool,
    /// 压缩、分支摘要、扩展提示这类旁注。
    Note,
}

#[derive(Debug, Clone)]
pub struct Tool {
    pub id: String,
    pub name: String,
    /// 参数摘要，一行。
    pub args: String,
    pub state: ToolState,
    /// 结果摘要，成功时是首行，失败时是失败原因。
    pub result: String,
    /// 完整结果正文，展开工具卡时显示。
    pub detail: String,
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolState {
    Running,
    Ok,
    Error,
}

/// 会话目录：`PI_CODING_AGENT_SESSION_DIR` 优先，其次 `~/.pi/agent/sessions`。
pub fn session_root() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("PI_CODING_AGENT_SESSION_DIR") {
        return Some(PathBuf::from(dir));
    }
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".pi/agent/sessions"))
}

/// pi 按 cwd 分目录：去掉开头的分隔符，把 `/`、`\`、`:` 换成 `-`，两侧再包 `--`。
pub fn session_dir(cwd: &str) -> Option<PathBuf> {
    let trimmed = cwd.trim_end_matches('/');
    let mangled: String = trimmed
        .trim_start_matches('/')
        .chars()
        .map(|ch| match ch {
            '/' | '\\' | ':' => '-',
            other => other,
        })
        .collect();
    Some(session_root()?.join(format!("--{mangled}--")))
}

/// 在 cwd 对应的目录里取最近写入的 `.jsonl`。
pub fn session_file(cwd: &str) -> Option<PathBuf> {
    let dir = session_dir(cwd)?;
    let entries = std::fs::read_dir(dir).ok()?;
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        if newest.as_ref().is_none_or(|(best, _)| modified > *best) {
            newest = Some((modified, path));
        }
    }
    newest.map(|(_, path)| path)
}

/// 顶栏显示的文件标签：`<时间>_<id>.jsonl` 里的 id 前 8 位。
pub fn file_label(path: &Path) -> String {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(t("session"));
    let stem = name.strip_suffix(".jsonl").unwrap_or(name);
    match stem.rsplit_once('_') {
        Some((_, id)) => format!("pi {}", &id[..id.len().min(8)]),
        None => format!("pi {stem}"),
    }
}

/// 把若干 JSONL 行切成消息块。
pub fn blocks(lines: &[String]) -> Vec<Block> {
    let entries: Vec<Value> = lines
        .iter()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect();
    let mut out = Vec::new();
    let mut tools: HashMap<String, usize> = HashMap::new();
    for value in chain(&entries) {
        match value.get("type").and_then(Value::as_str) {
            Some("message") => {
                if let Some(message) = value.get("message") {
                    push_message(&mut out, &mut tools, message);
                }
            }
            Some("compaction") => {
                let before = value
                    .get("tokensBefore")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                out.push(note(format!("上下文已压缩（压缩前 {before} tokens）")));
            }
            Some("branch_summary") => {
                let summary = value
                    .get("summary")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                out.push(note(format!("分支摘要：{}", one_line(summary, 120))));
            }
            _ => {}
        }
    }
    out
}

fn note(text: String) -> Block {
    Block {
        kind: BlockKind::Note,
        text,
        tool: None,
    }
}

/// 从当前叶子沿 `parentId` 走回根，返回正序的条目引用。
fn chain(entries: &[Value]) -> Vec<&Value> {
    let mut by_id: HashMap<&str, &Value> = HashMap::new();
    for entry in entries {
        if let Some(id) = entry.get("id").and_then(Value::as_str) {
            by_id.insert(id, entry);
        }
    }
    let mut leaf: Option<&Value> = None;
    for entry in entries {
        if entry.get("id").and_then(Value::as_str).is_some() {
            leaf = Some(entry);
        }
    }
    let mut out = Vec::new();
    let mut cursor = leaf;
    let mut guard = 0usize;
    while let Some(entry) = cursor {
        guard += 1;
        if guard > entries.len() + 1 {
            break;
        }
        out.push(entry);
        cursor = entry
            .get("parentId")
            .and_then(Value::as_str)
            .and_then(|parent| by_id.get(parent).copied());
    }
    out.reverse();
    out
}

fn push_message(out: &mut Vec<Block>, tools: &mut HashMap<String, usize>, message: &Value) {
    let role = message
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match role {
        "user" => {
            let text = content_text(message.get("content"));
            if !text.is_empty() {
                out.push(Block {
                    kind: BlockKind::User,
                    text,
                    tool: None,
                });
            }
        }
        "assistant" => {
            let mut text = String::new();
            let mut thinking = String::new();
            let mut calls = Vec::new();
            if let Some(blocks) = message.get("content").and_then(Value::as_array) {
                for block in blocks {
                    match block.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            if let Some(part) = block.get("text").and_then(Value::as_str) {
                                if !text.is_empty() {
                                    text.push('\n');
                                }
                                text.push_str(part);
                            }
                        }
                        Some("thinking") => {
                            if let Some(part) = block.get("thinking").and_then(Value::as_str) {
                                if !thinking.is_empty() {
                                    thinking.push('\n');
                                }
                                thinking.push_str(part);
                            }
                        }
                        Some("toolCall") => calls.push(block.clone()),
                        _ => {}
                    }
                }
            }
            if let Some(error) = message.get("errorMessage").and_then(Value::as_str) {
                if !error.is_empty() {
                    out.push(note(format!("错误：{}", one_line(error, 160))));
                }
            }
            if !thinking.trim().is_empty() {
                out.push(Block {
                    kind: BlockKind::Thinking,
                    text: thinking,
                    tool: None,
                });
            }
            if !text.trim().is_empty() {
                out.push(Block {
                    kind: BlockKind::Assistant,
                    text,
                    tool: None,
                });
            }
            for call in calls {
                let id = call
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let name = call
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or(t("tool"))
                    .to_string();
                let args = call
                    .get("arguments")
                    .map(argument_summary)
                    .unwrap_or_default();
                let tool = Tool {
                    id: id.clone(),
                    name,
                    args,
                    state: ToolState::Running,
                    result: String::new(),
                    detail: String::new(),
                    duration_ms: None,
                };
                if !id.is_empty() {
                    tools.insert(id, out.len());
                }
                out.push(Block {
                    kind: BlockKind::Tool,
                    text: String::new(),
                    tool: Some(tool),
                });
            }
        }
        "toolResult" => {
            let id = message
                .get("toolCallId")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let is_error = message
                .get("isError")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let body = content_text(message.get("content"));
            let (state, result) = if is_error {
                (ToolState::Error, one_line(&body, 160))
            } else {
                (ToolState::Ok, one_line(&body, 120))
            };
            match tools.get(id).and_then(|index| out.get_mut(*index)) {
                Some(Block {
                    tool: Some(tool), ..
                }) => {
                    tool.state = state;
                    tool.result = result;
                    tool.detail = clamp_text(&body, 8000);
                }
                _ => out.push(Block {
                    kind: BlockKind::Note,
                    text: format!("{} 结果：{result}", tool_name(message)),
                    tool: None,
                }),
            }
        }
        "bashExecution" => {
            let command = message
                .get("command")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let exit = message.get("exitCode").and_then(Value::as_i64);
            let tool = Tool {
                id: String::new(),
                name: "bash".to_string(),
                args: one_line(command, 100),
                state: match exit {
                    Some(0) => ToolState::Ok,
                    Some(_) => ToolState::Error,
                    None => ToolState::Ok,
                },
                result: match exit {
                    Some(code) => crate::i18n::t1("exit {}", code),
                    None => String::new(),
                },
                detail: String::new(),
                duration_ms: None,
            };
            out.push(Block {
                kind: BlockKind::Tool,
                text: String::new(),
                tool: Some(tool),
            });
        }
        "custom" => {
            let display = message
                .get("display")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let text = content_text(message.get("content"));
            if display && !text.trim().is_empty() {
                out.push(note(one_line(&text, 200)));
            }
        }
        "compactionSummary" | "branchSummary" => {
            let text = message
                .get("summary")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !text.trim().is_empty() {
                out.push(note(one_line(text, 200)));
            }
        }
        _ => {}
    }
}

fn tool_name(message: &Value) -> String {
    message
        .get("toolName")
        .and_then(Value::as_str)
        .unwrap_or(t("tool"))
        .to_string()
}

/// 文本块拼成多行；图片与未知块给一行占位。
fn content_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(blocks)) => {
            let mut parts = Vec::new();
            for block in blocks {
                match block.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        if let Some(text) = block.get("text").and_then(Value::as_str) {
                            parts.push(text.to_string());
                        }
                    }
                    Some("image") => parts.push("[图片]".to_string()),
                    _ => {}
                }
            }
            parts.join("\n")
        }
        _ => String::new(),
    }
}

/// 工具参数的一行摘要：优先取能说明动作的那个字段。
fn argument_summary(arguments: &Value) -> String {
    const KEYS: &[&str] = &[
        "command",
        "path",
        "file_path",
        "filePath",
        "pattern",
        "query",
        "url",
        "prompt",
        "description",
        "name",
    ];
    if let Some(map) = arguments.as_object() {
        for key in KEYS {
            if let Some(value) = map.get(*key) {
                if let Some(text) = value.as_str() {
                    return one_line(text, 100);
                }
            }
        }
        return one_line(&compact(map), 100);
    }
    one_line(&arguments.to_string(), 100)
}

fn compact(map: &Map<String, Value>) -> String {
    let mut out = String::from("{");
    for (index, (key, value)) in map.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        out.push_str(key);
        out.push(':');
        out.push_str(&value.to_string());
    }
    out.push('}');
    out
}

/// 首行 + 截断，用于所有单行摘要。
fn one_line(text: &str, limit: usize) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    let count = line.chars().count();
    let mut out: String = line.chars().take(limit).collect();
    if count > limit {
        out.push('…');
    }
    out
}

/// 完整正文只给展开视图：截到固定字符数，避免超长工具结果占着内存。
fn clamp_text(text: &str, limit: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let mut out: String = text.chars().take(limit).collect();
    out.push('…');
    out
}

#[cfg(test)]
/// 改 `PI_CODING_AGENT_SESSION_DIR` 的测试要串行，环境变量是进程级的。
pub static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    fn line(value: Value) -> String {
        value.to_string()
    }

    #[test]
    fn chain_follows_the_active_leaf_only() {
        let lines = vec![
            line(serde_json::json!({"type":"session","version":3,"id":"s"})),
            line(
                serde_json::json!({"type":"message","id":"a","parentId":null,
                "message":{"role":"user","content":"first"}}),
            ),
            line(serde_json::json!({"type":"message","id":"b","parentId":"a",
                "message":{"role":"assistant","content":[{"type":"text","text":"old"}]}})),
            line(serde_json::json!({"type":"message","id":"c","parentId":"a",
                "message":{"role":"user","content":"branch"}})),
        ];
        let blocks = blocks(&lines);
        let texts: Vec<&str> = blocks.iter().map(|block| block.text.as_str()).collect();
        assert_eq!(texts, vec!["first", "branch"]);
    }

    #[test]
    fn tool_result_patches_the_call() {
        let lines = vec![
            line(
                serde_json::json!({"type":"message","id":"a","parentId":null,
                "message":{"role":"assistant","content":[
                    {"type":"toolCall","id":"t1","name":"bash","arguments":{"command":"ls"}}]}}),
            ),
            line(serde_json::json!({"type":"message","id":"b","parentId":"a",
                "message":{"role":"toolResult","toolCallId":"t1","toolName":"bash",
                    "content":[{"type":"text","text":"a.rs\nb.rs"}],"isError":false}})),
        ];
        let blocks = blocks(&lines);
        assert_eq!(blocks.len(), 1);
        let tool = blocks[0].tool.as_ref().unwrap();
        assert_eq!(tool.name, "bash");
        assert_eq!(tool.args, "ls");
        assert_eq!(tool.state, ToolState::Ok);
        assert_eq!(tool.result, "a.rs");
    }

    #[test]
    fn session_dir_mangles_the_cwd() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|error| error.into_inner());
        std::env::set_var("PI_CODING_AGENT_SESSION_DIR", "/tmp/sessions");
        let dir = session_dir("/home/user/code/app").unwrap();
        assert_eq!(dir, PathBuf::from("/tmp/sessions/--home-user-code-app--"));
        std::env::remove_var("PI_CODING_AGENT_SESSION_DIR");
    }
}
