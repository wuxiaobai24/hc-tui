//! Chat 视图的数据面：跟随一个 pi 会话的 JSONL 文件，切成消息块。
//!
//! 终端仍然是唯一的真相来源：这里只读 pi 自己写的 transcript，输入仍走 Pane 的
//! attach 流回写。找不到会话文件时保留一条提示，不阻止其它视图。

pub mod commands;
pub mod hook;
pub mod pi;

use std::collections::BTreeSet;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use commands::Command;
use pi::{Block, ToolState};

/// 两次增量读取之间的最小间隔。
const POLL_INTERVAL: Duration = Duration::from_millis(120);
/// 检查 `session_file` 是否换到新会话的间隔（`/new`、`/resume` 之后）。
const SESSION_CHECK: Duration = Duration::from_millis(800);
/// 保留的原始行数上限，防止极长会话吃掉内存。
const MAX_LINES: usize = 6000;

/// Chat 底部的移动端动作条按钮。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatAction {
    Send,
    Interrupt,
    Palette,
    Terminal,
}

pub struct Chat {
    /// 这个 Chat 跟随的 Pane。
    pub pane: u32,
    /// Pane 的工作目录，用来定位与刷新会话文件。
    pub cwd: String,
    /// 会话文件；`None` 表示还没找到。
    pub path: Option<PathBuf>,
    /// 会话文件是 agent 自报的（`true`）还是按写入时间推断的（`false`）。
    pub exact: bool,
    /// Remote transcripts are read through core file.read, never local filesystem inference.
    pub machine: Option<String>,
    pub request_tag: String,
    remote_next_line: usize,
    /// 顶栏用的状态说明：会话标签、找不到的原因、读取错误。
    pub note: String,
    /// 由 transcript 切出来的消息块。
    pub blocks: Vec<Block>,
    /// 输入行内容。
    pub input: String,
    /// 光标在 `input` 里的字节下标。
    pub cursor: usize,
    /// 键盘选中的块下标。
    pub selected: Option<usize>,
    /// 展开的工具块（键是 tool id，`bashExecution` 用块下标）。
    pub expanded: BTreeSet<String>,
    /// 从底部往上滚过的行数。
    pub scroll: usize,
    /// 输入的发送历史，最新的在末尾。
    pub history: Vec<String>,
    /// 正在翻历史时的下标；`None` 表示不在翻历史。
    pub history_index: Option<usize>,
    /// 命令面板；输入以 `/` 开头时打开。
    pub palette: Option<Palette>,
    /// 上一次渲染算出的工具块行区域，鼠标点击用。
    pub hits: Vec<(usize, ratatui::layout::Rect)>,
    /// 上一次渲染算出的动作条按钮，鼠标点击用。
    pub actions: Vec<(ChatAction, ratatui::layout::Rect)>,
    /// 上一次渲染算出的消息区高度，按键滚动用。
    pub view_height: usize,
    /// 上一次渲染算出的视口起点（折行后的行号）与每个块的 `(块下标, 起行, 止行)`。
    pub view_start: usize,
    pub block_lines: Vec<(usize, usize, usize)>,
    offset: u64,
    partial: Vec<u8>,
    lines: Vec<String>,
    last_poll: Option<Instant>,
    last_session_check: Option<Instant>,
}

pub struct Palette {
    pub items: Vec<Command>,
    /// 过滤条件（输入行去掉前导 `/` 的部分）。
    pub query: String,
    /// 当前选中项。
    pub index: usize,
}

impl Chat {
    /// 按 cwd 找到 pi 会话文件并读入。`session` 是 agent 通过 label 自报的路径，
    /// 有就用它（确定映射），没有才按 cwd 目录里最新的文件推断。
    pub fn open(pane: u32, cwd: Option<&str>, session: Option<&str>) -> Self {
        let mut chat = Self::empty(pane, cwd);
        if let Some(path) = session.filter(|path| !path.is_empty()).map(PathBuf::from) {
            chat.adopt(path);
            return chat;
        }
        if chat.cwd.is_empty() {
            chat.note = "pane 没有 cwd".to_string();
            return chat;
        }
        match pi::session_file(&chat.cwd) {
            Some(path) => {
                chat.note = format!("{} · 本机推断", pi::file_label(&path));
                chat.path = Some(path);
                if let Err(error) = chat.reload() {
                    chat.note = format!("读取会话失败：{error}");
                }
            }
            None => chat.note = format!("{} 下没有本机 pi 会话", chat.cwd),
        }
        chat
    }

    fn empty(pane: u32, cwd: Option<&str>) -> Self {
        let cwd = cwd.unwrap_or_default().to_string();
        Self {
            pane,
            cwd,
            path: None,
            exact: false,
            machine: None,
            request_tag: crate::shared::identity("chat:"),
            remote_next_line: 1,
            note: String::new(),
            blocks: Vec::new(),
            input: String::new(),
            cursor: 0,
            selected: None,
            expanded: BTreeSet::new(),
            scroll: 0,
            history: Vec::new(),
            history_index: None,
            palette: None,
            hits: Vec::new(),
            actions: Vec::new(),
            view_height: 0,
            view_start: 0,
            block_lines: Vec::new(),
            offset: 0,
            partial: Vec::new(),
            lines: Vec::new(),
            last_poll: None,
            last_session_check: None,
        }
    }

    pub fn open_remote(pane: u32, machine: &str, cwd: Option<&str>, session: Option<&str>) -> Self {
        let mut chat = Self::empty(pane, cwd);
        chat.machine = Some(machine.to_string());
        chat.set_remote_session(session);
        chat
    }

    /// A missing remote label must never fall back to a same-named local file.
    pub fn set_remote_session(&mut self, session: Option<&str>) {
        let path = session.filter(|path| !path.is_empty()).map(PathBuf::from);
        if self.path == path && !self.note.is_empty() {
            return;
        }
        self.path = path;
        self.exact = self.path.is_some();
        self.remote_next_line = 1;
        self.last_poll = None;
        self.partial.clear();
        self.lines.clear();
        self.rebuild();
        let machine = self.machine.as_deref().unwrap_or_default();
        self.note = match &self.path {
            Some(path) => format!("{machine} · 远端映射 · {} · 读取中", pi::file_label(path)),
            None => format!("{machine} · 远端会话未映射；需在该机器安装 session hook"),
        };
    }

    pub fn remote_request(&mut self) -> Option<(String, serde_json::Value)> {
        let machine = self.machine.as_ref()?;
        let path = self.path.as_ref()?;
        let now = Instant::now();
        if self
            .last_poll
            .is_some_and(|at| now.duration_since(at) < SESSION_CHECK)
        {
            return None;
        }
        self.last_poll = Some(now);
        Some((
            self.request_tag.clone(),
            serde_json::json!({
                "on": machine, "path":path, "offset":self.remote_next_line.saturating_sub(1).max(1), "limit": MAX_LINES,
            }),
        ))
    }

    pub fn remote_reply(
        &mut self,
        tag: &str,
        request: &serde_json::Value,
        result: Result<serde_json::Value, String>,
    ) -> bool {
        if tag != self.request_tag
            || self.machine.as_deref() != request["on"].as_str()
            || self.path.as_ref().and_then(|p| p.to_str()) != request["path"].as_str()
        {
            return false;
        }
        let machine = self.machine.clone().unwrap_or_default();
        let value = match result {
            Ok(value) => value,
            Err(error) => {
                self.note = format!("{machine} · 远端读取失败：{error}");
                return true;
            }
        };
        let Some(text) = value["text"].as_str() else {
            self.note = format!("{machine} · 远端读取失败：响应缺少 text");
            return true;
        };
        let total = value["lines"].as_u64().unwrap_or(0) as usize;
        let first = value["first"].as_u64().unwrap_or(1) as usize;
        if total < self.remote_next_line.saturating_sub(1) {
            self.lines.clear();
            self.remote_next_line = 1;
            self.rebuild();
            self.last_poll = None;
            return true;
        }
        // Start a long transcript at its bounded tail rather than repeatedly transferring it all.
        if self.remote_next_line == 1 && total > MAX_LINES {
            self.remote_next_line = total - MAX_LINES + 1;
            self.last_poll = None;
            return true;
        }
        let mut partial = Vec::new();
        let mut incoming = split_lines(&mut partial, text.as_bytes(), false);
        if first < self.remote_next_line && !incoming.is_empty() {
            let anchor = incoming.remove(0);
            if self.lines.last().is_some_and(|last| *last != anchor) {
                self.lines.clear();
                self.remote_next_line = 1;
                self.rebuild();
                self.last_poll = None;
                return true;
            }
        }
        let count = incoming.len();
        self.remote_next_line += count;
        self.lines.extend(incoming);
        if self.lines.len() > MAX_LINES {
            self.lines.drain(..self.lines.len() - MAX_LINES);
        }
        self.rebuild();
        self.note = format!(
            "{machine} · 远端映射 · {}",
            self.path
                .as_ref()
                .map(|p| pi::file_label(p))
                .unwrap_or_default()
        );
        true
    }

    /// 换成 agent 自报的会话文件（label 到了之后调用）。
    pub fn adopt(&mut self, path: PathBuf) {
        if Some(&path) == self.path.as_ref() && self.exact {
            return;
        }
        self.note = format!("{} · 本机映射", pi::file_label(&path));
        self.path = Some(path);
        self.exact = true;
        self.offset = 0;
        self.partial.clear();
        self.lines.clear();
        self.rebuild();
        if let Err(error) = self.reload() {
            self.note = format!("读取会话失败：{error}");
        }
    }

    pub fn tool_running(&self) -> bool {
        self.blocks.iter().any(|block| {
            block
                .tool
                .as_ref()
                .is_some_and(|tool| tool.state == ToolState::Running)
        })
    }

    /// 最后一条助手正文，`Ctrl+Y` 复制用。
    pub fn last_reply(&self) -> Option<&str> {
        self.blocks
            .iter()
            .rev()
            .find(|block| block.kind == pi::BlockKind::Assistant)
            .map(|block| block.text.as_str())
    }

    /// 工具块的展开键：有 tool id 用它，否则用块下标。
    pub fn tool_key(&self, index: usize) -> String {
        match self
            .blocks
            .get(index)
            .and_then(|block| block.tool.as_ref())
            .map(|tool| tool.id.as_str())
        {
            Some(id) if !id.is_empty() => id.to_string(),
            _ => format!("#{index}"),
        }
    }

    pub fn is_expanded(&self, index: usize) -> bool {
        self.expanded.contains(&self.tool_key(index))
    }

    pub fn toggle_expanded(&mut self, index: usize) {
        let key = self.tool_key(index);
        if !self.expanded.remove(&key) {
            self.expanded.insert(key);
        }
    }

    pub fn selected_tool(&self) -> Option<usize> {
        let index = self.selected?;
        match self.blocks.get(index) {
            Some(block) if block.tool.is_some() => Some(index),
            _ => None,
        }
    }

    /// `↑`/`↓`：在块之间移动选中项。
    pub fn select(&mut self, delta: isize) {
        if self.blocks.is_empty() {
            self.selected = None;
            return;
        }
        let last = self.blocks.len() - 1;
        let next = match self.selected {
            None => {
                if delta > 0 {
                    0
                } else {
                    last
                }
            }
            Some(current) => {
                let moved = current as isize + delta;
                moved.clamp(0, last as isize) as usize
            }
        };
        self.selected = Some(next);
    }

    /// 把选中块滚进视口：`↑`/`↓` 之后保证看得见。
    pub fn reveal_selected(&mut self) {
        let Some(index) = self.selected else {
            return;
        };
        let Some((_, first, last)) = self
            .block_lines
            .iter()
            .find(|(block, _, _)| *block == index)
            .copied()
        else {
            return;
        };
        let height = self.view_height.max(1);
        let start = self.view_start;
        if first < start {
            self.scroll = self.scroll.saturating_add(start - first);
        } else if last >= start + height {
            self.scroll = self.scroll.saturating_sub(last + 1 - (start + height));
        }
    }

    /// 打开命令面板：内置命令 + 当前项目里发现的模板与技能。
    pub fn open_palette(&mut self) {
        let items = if self.machine.is_some() {
            commands::builtin()
        } else {
            commands::all(&self.cwd)
        };
        let query = self.input.strip_prefix('/').unwrap_or_default().to_string();
        self.palette = Some(Palette {
            items,
            query,
            index: 0,
        });
    }

    pub fn close_palette(&mut self) {
        self.palette = None;
    }

    /// 输入变化后同步面板的过滤条件；输入不再是 `/` 开头就关掉。
    pub fn sync_palette(&mut self) {
        let Some(stem) = self.input.strip_prefix('/') else {
            self.palette = None;
            return;
        };
        let query = stem.to_string();
        if self.palette.is_none() {
            self.open_palette();
        }
        if let Some(palette) = &mut self.palette {
            palette.query = query;
            let count = commands::filter(&palette.items, &palette.query).len();
            if palette.index >= count {
                palette.index = count.saturating_sub(1);
            }
        }
    }

    /// 面板当前过滤出的条目下标。
    pub fn palette_matches(&self) -> Vec<usize> {
        match &self.palette {
            Some(palette) => commands::filter(&palette.items, &palette.query),
            None => Vec::new(),
        }
    }

    pub fn palette_move(&mut self, delta: isize) {
        let matches = self.palette_matches();
        let Some(palette) = &mut self.palette else {
            return;
        };
        if matches.is_empty() {
            palette.index = 0;
            return;
        }
        let last = matches.len() - 1;
        let moved = palette.index as isize + delta;
        palette.index = moved.clamp(0, last as isize) as usize;
    }

    pub fn palette_selected(&self) -> Option<Command> {
        let matches = self.palette_matches();
        let palette = self.palette.as_ref()?;
        matches
            .get(palette.index)
            .and_then(|index| palette.items.get(*index))
            .cloned()
    }

    /// 输入行里插入文本并同步面板。
    pub fn insert(&mut self, text: &str) {
        let text = text.replace(['\n', '\r'], " ");
        if text.is_empty() {
            return;
        }
        self.input.insert_str(self.cursor, &text);
        self.cursor += text.len();
        self.history_index = None;
        self.sync_palette();
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let start = crate::app::prev_char_boundary(&self.input, self.cursor);
        self.input.replace_range(start..self.cursor, "");
        self.cursor = start;
        self.history_index = None;
        self.sync_palette();
    }

    pub fn delete(&mut self) {
        if self.cursor >= self.input.len() {
            return;
        }
        let end = crate::app::next_char_boundary(&self.input, self.cursor);
        self.input.replace_range(self.cursor..end, "");
        self.sync_palette();
    }

    pub fn move_cursor(&mut self, delta: isize) {
        self.cursor = if delta < 0 {
            crate::app::prev_char_boundary(&self.input, self.cursor)
        } else {
            crate::app::next_char_boundary(&self.input, self.cursor)
        };
    }

    pub fn clear_input(&mut self) {
        self.input.clear();
        self.cursor = 0;
        self.history_index = None;
        self.palette = None;
    }

    /// 翻输入历史：`delta < 0` 往更早的翻。
    pub fn recall(&mut self, delta: isize) {
        if self.history.is_empty() {
            return;
        }
        let last = self.history.len() - 1;
        let index = match self.history_index {
            None => {
                if delta < 0 {
                    last
                } else {
                    return;
                }
            }
            Some(current) => {
                let moved = current as isize + delta;
                if moved < 0 {
                    0
                } else if moved > last as isize {
                    self.history_index = None;
                    self.clear_input();
                    return;
                } else {
                    moved as usize
                }
            }
        };
        self.history_index = Some(index);
        self.input = self.history[index].clone();
        self.cursor = self.input.len();
        self.sync_palette();
    }

    /// 把输入行交给调用方发送，并清空输入行。
    pub fn take_input(&mut self) -> Option<String> {
        let text = self.input.trim_end().to_string();
        if text.trim().is_empty() {
            self.clear_input();
            return None;
        }
        if self.history.last().map(String::as_str) != Some(text.as_str()) {
            self.history.push(text.clone());
            if self.history.len() > 64 {
                self.history.remove(0);
            }
        }
        self.clear_input();
        self.scroll = 0;
        Some(text)
    }

    /// 整文件重读（首次打开、文件被截断或重写）。
    fn reload(&mut self) -> std::io::Result<()> {
        let Some(path) = self.path.clone() else {
            return Ok(());
        };
        let mut file = std::fs::File::open(&path)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        self.offset = bytes.len() as u64;
        self.partial.clear();
        self.lines = split_lines(&mut self.partial, &bytes, false);
        if self.lines.len() > MAX_LINES {
            self.lines.drain(..self.lines.len() - MAX_LINES);
        }
        self.rebuild();
        Ok(())
    }

    /// 增量读文件；有变化返回 `true`。
    pub fn poll(&mut self) -> bool {
        if self.machine.is_some() {
            return false;
        }
        let changed = self.check_session();
        let Some(path) = self.path.clone() else {
            return changed;
        };
        let now = Instant::now();
        if self
            .last_poll
            .is_some_and(|last| now.duration_since(last) < POLL_INTERVAL)
        {
            return changed;
        }
        self.last_poll = Some(now);
        let Ok(meta) = std::fs::metadata(&path) else {
            return changed;
        };
        let len = meta.len();
        if len < self.offset {
            // 会话被重写（新建、切换分支后的重放），整体重来。
            return self.reload().is_ok() || changed;
        }
        if len == self.offset {
            return changed;
        }
        let Ok(mut file) = std::fs::File::open(&path) else {
            return changed;
        };
        if file.seek(SeekFrom::Start(self.offset)).is_err() {
            return changed;
        }
        let mut bytes = Vec::new();
        if file.read_to_end(&mut bytes).is_err() {
            return changed;
        }
        self.offset += bytes.len() as u64;
        let lines = split_lines(&mut self.partial, &bytes, false);
        if lines.is_empty() {
            return changed;
        }
        self.lines.extend(lines);
        if self.lines.len() > MAX_LINES {
            let drop = self.lines.len() - MAX_LINES;
            self.lines.drain(..drop);
        }
        self.rebuild();
        true
    }

    /// `/new`、`/resume` 会换会话文件，定期看一眼目录里最新的是不是同一份。
    fn check_session(&mut self) -> bool {
        if self.cwd.is_empty() {
            return false;
        }
        let now = Instant::now();
        if self
            .last_session_check
            .is_some_and(|last| now.duration_since(last) < SESSION_CHECK)
        {
            return false;
        }
        self.last_session_check = Some(now);
        let Some(path) = pi::session_file(&self.cwd) else {
            return false;
        };
        if self.exact || Some(&path) == self.path.as_ref() {
            return false;
        }
        self.path = Some(path.clone());
        self.note = format!("{} · 本机推断", pi::file_label(&path));
        self.offset = 0;
        self.partial.clear();
        self.lines.clear();
        let _ = self.reload();
        true
    }

    fn rebuild(&mut self) {
        self.blocks = pi::blocks(&self.lines);
        if self.blocks.is_empty() {
            self.selected = None;
        } else if self
            .selected
            .is_some_and(|index| index >= self.blocks.len())
        {
            self.selected = Some(self.blocks.len() - 1);
        }
        self.hits.clear();
        self.block_lines.clear();
    }
}

#[cfg(test)]
impl Chat {
    /// 测试用：不走文件，直接喂 JSONL 行。
    pub fn from_lines(pane: u32, lines: Vec<String>) -> Self {
        let mut chat = Self::open(pane, None, None);
        chat.cwd = String::new();
        chat.note = "pi test".to_string();
        chat.lines = lines;
        chat.rebuild();
        chat
    }
}

/// 把一段字节切成完整行，尾部半行留在 `partial` 里。
fn split_lines(partial: &mut Vec<u8>, bytes: &[u8], complete: bool) -> Vec<String> {
    let mut buffer = std::mem::take(partial);
    buffer.extend_from_slice(bytes);
    let mut lines = Vec::new();
    let mut start = 0;
    for (index, byte) in buffer.iter().enumerate() {
        if *byte != b'\n' {
            continue;
        }
        let end = if index > start && buffer[index - 1] == b'\r' {
            index - 1
        } else {
            index
        };
        lines.push(String::from_utf8_lossy(&buffer[start..end]).into_owned());
        start = index + 1;
    }
    let rest = buffer[start..].to_vec();
    if complete && !rest.is_empty() {
        lines.push(String::from_utf8_lossy(&rest).into_owned());
        partial.clear();
    } else {
        *partial = rest;
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::{pi, split_lines, Chat};
    use std::io::Write;

    fn temp_root(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("hc-tui-chat-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp root");
        dir
    }

    fn append(path: &std::path::Path, line: &str) {
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .expect("session file");
        writeln!(file, "{line}").expect("append");
    }

    #[test]
    fn remote_transcript_never_falls_back_to_local_files_and_ignores_stale_replies() {
        let root = temp_root("remote");
        let path = root.join("session.jsonl");
        append(
            &path,
            r#"{"type":"message","id":"local","parentId":null,"message":{"role":"user","content":"local decoy"}}"#,
        );
        let mut chat = Chat::open_remote(7, "dev", Some("/repo"), Some(path.to_str().unwrap()));
        assert!(chat.blocks.is_empty());
        assert!(!chat.poll());
        let (tag, request) = chat.remote_request().unwrap();
        assert_eq!(request["on"], "dev");
        let text = "{\"type\":\"message\",\"id\":\"a\",\"parentId\":null,\"message\":{\"role\":\"user\",\"content\":\"remote message\"}}\n";
        assert!(chat.remote_reply(
            &tag,
            &request,
            Ok(serde_json::json!({"first":1,"last":1,"lines":1,"text":text}))
        ));
        assert_eq!(chat.blocks[0].text, "remote message");
        chat.set_remote_session(Some("/other/session.jsonl"));
        assert!(chat.blocks.is_empty());
        assert!(!chat.remote_reply(&tag, &request, Ok(serde_json::json!({"text":text}))));
        chat.set_remote_session(None);
        assert!(chat.remote_request().is_none());
        assert!(chat.note.contains("远端会话未映射"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn remote_line_polling_handles_partial_appends_truncation_and_errors() {
        let mut chat = Chat::open_remote(3, "peer", Some("/repo"), Some("/session.jsonl"));
        let (tag, request) = chat.remote_request().unwrap();
        let a = r#"{"type":"message","id":"a","parentId":null,"message":{"role":"user","content":"first"}}"#;
        let b = r#"{"type":"message","id":"b","parentId":"a","message":{"role":"assistant","content":[{"type":"text","text":"second"}]}}"#;
        chat.remote_reply(&tag,&request,Ok(serde_json::json!({"first":1,"last":2,"lines":2,"text":format!("{a}\n{}",&b[..30])})));
        assert_eq!(chat.blocks.len(), 1);
        chat.last_poll = None;
        let (_, request) = chat.remote_request().unwrap();
        assert_eq!(request["offset"], 1);
        chat.remote_reply(
            &tag,
            &request,
            Ok(serde_json::json!({"first":1,"last":2,"lines":2,"text":format!("{a}\n{b}\n")})),
        );
        assert_eq!(chat.blocks.len(), 2);
        assert_eq!(chat.remote_next_line, 3);
        chat.remote_reply(&tag, &request, Err("machine unavailable".into()));
        assert!(chat.note.contains("远端读取失败"));
        assert_eq!(chat.blocks.len(), 2);
        chat.remote_reply(
            &tag,
            &request,
            Ok(serde_json::json!({"first":2,"last":0,"lines":0,"text":""})),
        );
        assert!(chat.blocks.is_empty());
        assert_eq!(chat.remote_next_line, 1);
    }

    #[test]
    fn a_reported_session_wins_over_the_newest_file() {
        let _guard = pi::ENV_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let root = temp_root("label");
        std::env::set_var("PI_CODING_AGENT_SESSION_DIR", &root);
        let cwd = "/tmp/app";
        let dir = root.join("--tmp-app--");
        std::fs::create_dir_all(&dir).expect("session dir");
        let heuristic = dir.join("2026-02-02T00-00-00Z_heuristic.jsonl");
        append(
            &heuristic,
            r#"{"type":"session","version":3,"id":"heuristic","cwd":"/tmp/app"}"#,
        );
        append(
            &heuristic,
            r#"{"type":"message","id":"a","parentId":null,"message":{"role":"user","content":"newest"}}"#,
        );
        // 文件系统时间戳的粒度可能很粗，等一下再写第二份，保证它更新。
        std::thread::sleep(std::time::Duration::from_millis(20));
        let reported = dir.join("2026-01-01T00-00-00Z_reported.jsonl");
        append(
            &reported,
            r#"{"type":"session","version":3,"id":"reported","cwd":"/tmp/app"}"#,
        );
        append(
            &reported,
            r#"{"type":"message","id":"a","parentId":null,"message":{"role":"user","content":"labelled"}}"#,
        );

        let chat = Chat::open(3, Some(cwd), Some(&reported.to_string_lossy()));
        assert!(chat.exact);
        assert_eq!(chat.blocks.len(), 1);
        assert_eq!(chat.blocks[0].text, "labelled");

        // 没有自报路径时按最新文件推断，并在顶栏标注。
        std::thread::sleep(std::time::Duration::from_millis(20));
        append(
            &heuristic,
            r#"{"type":"message","id":"b","parentId":"a","message":{"role":"assistant","content":[{"type":"text","text":"newest"}]}}"#,
        );
        let mut chat = Chat::open(3, Some(cwd), None);
        assert!(!chat.exact);
        assert_eq!(
            chat.path.as_ref().and_then(|path| path.file_name()),
            heuristic.file_name()
        );
        assert!(chat.note.ends_with("推断"), "{}", chat.note);

        // label 晚到时切过去，并且不再被推断逻辑改回。
        chat.adopt(reported.clone());
        assert!(chat.exact);
        assert_eq!(chat.blocks[0].text, "labelled");
        chat.last_session_check = None;
        chat.poll();
        assert_eq!(chat.path.as_ref(), Some(&reported));

        let _ = std::fs::remove_dir_all(&root);
        std::env::remove_var("PI_CODING_AGENT_SESSION_DIR");
    }

    #[test]
    fn poll_follows_appends_and_new_sessions() {
        let _guard = pi::ENV_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let root = temp_root("poll");
        std::env::set_var("PI_CODING_AGENT_SESSION_DIR", &root);
        let cwd = "/tmp/app";
        let dir = root.join("--tmp-app--");
        std::fs::create_dir_all(&dir).expect("session dir");
        let first = dir.join("2026-01-01T00-00-00Z_aaa.jsonl");
        append(
            &first,
            r#"{"type":"session","version":3,"id":"aaa","cwd":"/tmp/app"}"#,
        );
        append(
            &first,
            r#"{"type":"message","id":"a","parentId":null,"message":{"role":"user","content":"hi"}}"#,
        );
        let mut chat = Chat::open(7, Some(cwd), None);
        assert_eq!(chat.blocks.len(), 1);
        assert_eq!(chat.blocks[0].text, "hi");

        append(
            &first,
            r#"{"type":"message","id":"b","parentId":"a","message":{"role":"assistant","content":[{"type":"text","text":"hello"}]}}"#,
        );
        chat.last_poll = None;
        chat.last_session_check = None;
        assert!(chat.poll());
        assert_eq!(chat.blocks.len(), 2);
        assert_eq!(chat.blocks[1].text, "hello");

        // /new 之后目录里多一份更新的会话：Chat 跟过去。
        let second = dir.join("2026-01-02T00-00-00Z_bbb.jsonl");
        append(
            &second,
            r#"{"type":"session","version":3,"id":"bbb","cwd":"/tmp/app"}"#,
        );
        append(
            &second,
            r#"{"type":"message","id":"a","parentId":null,"message":{"role":"user","content":"next"}}"#,
        );
        chat.last_session_check = None;
        assert!(chat.poll());
        assert_eq!(chat.path.as_ref(), Some(&second));
        assert_eq!(chat.blocks.len(), 1);
        assert_eq!(chat.blocks[0].text, "next");

        let _ = std::fs::remove_dir_all(&root);
        std::env::remove_var("PI_CODING_AGENT_SESSION_DIR");
    }

    #[test]
    fn split_keeps_the_partial_tail() {
        let mut partial = Vec::new();
        let lines = split_lines(&mut partial, b"{\"a\":1}\n{\"b\"", false);
        assert_eq!(lines, vec!["{\"a\":1}".to_string()]);
        assert_eq!(partial, b"{\"b\"");
        let lines = split_lines(&mut partial, b":2}\r\n", false);
        assert_eq!(lines, vec!["{\"b\":2}".to_string()]);
        assert!(partial.is_empty());
    }
}
