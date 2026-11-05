//! Chat 里能发起的命令：pi 的内置斜杠命令，加上项目与会话里发现的模板和技能。
//!
//! 命令只是回写到终端的文本，pi 自己的处理逻辑不受影响。需要 TUI 浮层的命令
//! （设置、会话选择器、登录等）标成 `terminal`，采纳后切回终端视图。

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    /// 带前导 `/` 的命令文本；本地动作是给人看的名字。
    pub name: String,
    pub description: String,
    pub kind: Kind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// 直接发进终端。
    Prompt,
    /// 需要 pi 的终端浮层：采纳后切回终端，由用户接着操作。
    Terminal,
    /// hc-tui 自己执行的动作，不发给 pi。
    Local(Local),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Local {
    InstallHook,
    RemoveHook,
}

impl Command {
    fn new(name: &str, description: &str, terminal: bool) -> Self {
        Self {
            name: name.to_string(),
            description: description.to_string(),
            kind: if terminal {
                Kind::Terminal
            } else {
                Kind::Prompt
            },
        }
    }

    fn local(name: &str, description: &str, action: Local) -> Self {
        Self {
            name: name.to_string(),
            description: description.to_string(),
            kind: Kind::Local(action),
        }
    }
}

/// pi 内置命令；`terminal` 表示它会打开 TUI 浮层或需要确认。
pub fn builtin() -> Vec<Command> {
    vec![
        Command::new("/settings", "打开设置", true),
        Command::new("/model", "选择模型", true),
        Command::new("/thinking", "设置思考深度", true),
        Command::new("/scoped-models", "配置轮换模型", true),
        Command::new("/login", "添加供应商登录", true),
        Command::new("/logout", "移除供应商登录", true),
        Command::new("/llama", "管理 llama.cpp 模型", true),
        Command::new("/new", "新建会话", false),
        Command::new("/resume", "切换到另一个会话", true),
        Command::new("/name", "设置会话名", false),
        Command::new("/session", "会话信息与统计", false),
        Command::new("/tree", "在会话树里跳转", true),
        Command::new("/fork", "从某条消息分叉", true),
        Command::new("/clone", "复制当前会话", false),
        Command::new("/compact", "压缩上下文", false),
        Command::new("/import", "导入 JSONL 会话", true),
        Command::new("/copy", "复制最后一条回复", false),
        Command::new("/export", "导出会话", false),
        Command::new("/share", "上传会话并取链接", true),
        Command::new("/bug", "准备 bug 报告", true),
        Command::new("/trust", "保存项目信任", true),
        Command::new("/reload", "重载扩展与配置", false),
        Command::new("/hotkeys", "查看快捷键", true),
        Command::new("/changelog", "查看更新日志", true),
        Command::new("/quit", "退出 pi", false),
    ]
}

/// 当前工作目录能用的全部命令：内置 + prompt 模板 + 技能 + hc-tui 自己的动作。
pub fn all(cwd: &str) -> Vec<Command> {
    let mut items = builtin();
    for (name, description) in templates(cwd) {
        items.push(Command::new(&format!("/{name}"), &description, false));
    }
    for (name, description) in skills(cwd) {
        items.push(Command::new(&format!("/skill:{name}"), &description, false));
    }
    match super::hook::status() {
        super::hook::Status::Current => items.push(Command::local(
            "移除会话映射 hook",
            "删掉 pi 扩展，映射退回按时间推断",
            Local::RemoveHook,
        )),
        _ => items.push(Command::local(
            "安装会话映射 hook",
            "让 pi 自报会话文件，映射从推断变确定",
            Local::InstallHook,
        )),
    }
    items
}

/// 按名字与描述做大小写不敏感的子串过滤，返回命中的下标。
pub fn filter(items: &[Command], query: &str) -> Vec<usize> {
    let query = query.trim().to_ascii_lowercase();
    items
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            if query.is_empty() {
                return true;
            }
            let name = item.name.trim_start_matches('/').to_ascii_lowercase();
            name.contains(&query) || item.description.to_lowercase().contains(&query)
        })
        .map(|(index, _)| index)
        .collect()
}

/// prompt 模板：`~/.pi/agent/prompts/*.md` 与项目的 `.pi/prompts/*.md`。
fn templates(cwd: &str) -> Vec<(String, String)> {
    let mut dirs = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join(".pi/agent/prompts"));
    }
    if !cwd.is_empty() {
        dirs.push(Path::new(cwd).join(".pi/prompts"));
    }
    markdown_names(&dirs, "")
}

/// 技能：`/skill:name`，来自 pi 与 Agent Skills 的标准位置。
fn skills(cwd: &str) -> Vec<(String, String)> {
    let mut dirs = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        dirs.push(home.join(".pi/agent/skills"));
        dirs.push(home.join(".agents/skills"));
    }
    if !cwd.is_empty() {
        let mut path = Some(Path::new(cwd));
        while let Some(dir) = path {
            dirs.push(dir.join(".pi/skills"));
            dirs.push(dir.join(".agents/skills"));
            if dir.join(".git").exists() {
                break;
            }
            path = dir.parent();
        }
    }
    markdown_names(&dirs, "SKILL.md")
}

/// 从若干目录里取 `<dir>/<name>/<file>` 形式的条目；`file` 为空时取 `*.md`。
fn markdown_names(dirs: &[PathBuf], file: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if file.is_empty() {
                if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
                    continue;
                }
                if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
                    out.push((stem.to_string(), String::new()));
                }
                continue;
            }
            let candidate = path.join(file);
            if !candidate.exists() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            out.push((name.to_string(), front_matter(&candidate)));
        }
    }
    out.sort_by(|left, right| left.0.cmp(&right.0));
    out.dedup_by(|left, right| left.0 == right.0);
    out
}

/// 读 SKILL.md 顶部 front matter 的 `description`，取不到就用空串。
fn front_matter(path: &Path) -> String {
    let Ok(text) = std::fs::read_to_string(path) else {
        return String::new();
    };
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return String::new();
    }
    for line in lines {
        let line = line.trim();
        if line == "---" {
            break;
        }
        if let Some(value) = line.strip_prefix("description:") {
            return value.trim().trim_matches('"').to_string();
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_matches_name_and_description() {
        let items = vec![
            Command::new("/model", "选择模型", true),
            Command::new("/new", "新建会话", false),
        ];
        assert_eq!(filter(&items, "mod"), vec![0]);
        assert_eq!(filter(&items, "新建"), vec![1]);
        assert_eq!(filter(&items, "").len(), 2);
    }
}
