//! hc 协议里本客户端用到的数据结构。
//!
//! 全部按「缺字段就用默认值」解析：core 0.x 会加字段，客户端不能因此拒收。
//! 依据：docs/protocol-facts.md §4、§6、§9。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 一份终端记录中本客户端需要展示的部分。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalSummary {
    pub address: String,
    pub name: Option<String>,
    pub title: String,
    pub cols: u16,
    pub rows: u16,
    pub live: bool,
    /// OSC 7501 上报的状态词。`None` 表示从未上报，**不得**推断为 idle。
    pub status: Option<String>,
    /// 丢失后是否用 shell 恢复。
    pub keep: bool,
}

impl TerminalSummary {
    pub fn parse(value: &Value) -> Option<Self> {
        let address = value.get("address")?.as_str()?.to_string();
        let string = |key: &str| {
            value
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_string)
                .filter(|s| !s.is_empty())
        };
        let number = |key: &str| value.get(key).and_then(Value::as_u64).unwrap_or(0) as u16;
        Some(Self {
            address,
            name: string("name"),
            title: string("title").unwrap_or_default(),
            cols: number("cols"),
            rows: number("rows"),
            live: value.get("state").and_then(Value::as_str) != Some("finished"),
            // `status` 是 `{state, kind?, pct?, msg?}`；旧版可能直接给字符串。
            status: value
                .get("status")
                .and_then(|status| status.get("state").and_then(Value::as_str))
                .or_else(|| value.get("status").and_then(Value::as_str))
                .map(str::to_string)
                .filter(|status| !status.is_empty()),
            keep: value.get("keep").and_then(Value::as_bool).unwrap_or(false),
        })
    }

    /// 侧栏与 Pane 标题用的名字：显式名字优先，其次标题，最后地址。
    pub fn label(&self) -> &str {
        self.name
            .as_deref()
            .filter(|name| !name.is_empty())
            .or_else(|| (!self.title.is_empty()).then_some(self.title.as_str()))
            .unwrap_or(&self.address)
    }
}

/// `events.subscribe` 的返回。
#[derive(Debug, Clone)]
pub struct SubscribeResult {
    pub seq: u64,
    pub channel: Option<u32>,
    pub terminals: Vec<TerminalSummary>,
}

impl SubscribeResult {
    pub fn parse(value: &Value) -> Option<Self> {
        Some(Self {
            seq: value.get("seq").and_then(Value::as_u64).unwrap_or(0),
            channel: value
                .get("channel")
                .and_then(Value::as_u64)
                .map(|channel| channel as u32),
            terminals: terminals_of(value),
        })
    }
}

/// `terminal.attach` 的返回。
#[derive(Debug, Clone)]
pub struct AttachResult {
    pub channel: u32,
    pub resize_notifications: bool,
    pub terminal: Option<TerminalSummary>,
}

impl AttachResult {
    pub fn parse(value: &Value) -> Option<Self> {
        Some(Self {
            channel: value.get("channel")?.as_u64()? as u32,
            resize_notifications: value
                .get("resize_notifications")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            terminal: value.get("terminal").and_then(TerminalSummary::parse),
        })
    }
}

/// events channel 上的一行事件。`type` 是关键字，所以字段名叫 `kind`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub kind: String,
    pub terminal: Option<String>,
    pub seq: u64,
    pub data: Value,
}

impl Event {
    /// `params` 是 `{"method":"event","params":{...}}` 里的内层对象。
    pub fn parse(params: &Value) -> Option<Self> {
        Some(Self {
            kind: params.get("type")?.as_str()?.to_string(),
            terminal: params
                .get("terminal")
                .and_then(Value::as_str)
                .map(str::to_string),
            seq: params.get("seq").and_then(Value::as_u64).unwrap_or(0),
            data: params.get("data").cloned().unwrap_or(Value::Null),
        })
    }

    /// `missed` 事件携带全量快照，要求整体重同步。
    pub fn is_resync(&self) -> bool {
        self.kind == "missed"
    }
}

/// viewer 自带终端的属性，attach 的 `display` 参数与 FACTS 帧共用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Display {
    pub fg: Option<String>,
    pub bg: Option<String>,
    pub cursor: Option<String>,
    pub cell: Option<[u32; 2]>,
}

impl Display {
    /// FACTS 只需要 cell 像素尺寸；颜色留空让 core 用自己的默认值。
    pub fn with_cell(width: u32, height: u32) -> Self {
        Self {
            fg: None,
            bg: None,
            cursor: None,
            cell: Some([width, height]),
        }
    }
}

#[derive(Debug, Deserialize)]
struct SnapshotShape {
    #[serde(default)]
    terminals: Vec<Value>,
}

fn terminals_of(value: &Value) -> Vec<TerminalSummary> {
    serde_json::from_value::<SnapshotShape>(value.clone())
        .map(|snapshot| {
            snapshot
                .terminals
                .iter()
                .filter_map(TerminalSummary::parse)
                .collect()
        })
        .unwrap_or_default()
}

/// attach 流 CLOSE 帧的 payload 是 JSON reason；解析不出来就当原始文本。
pub fn close_reason(payload: &[u8]) -> String {
    if payload.is_empty() {
        return "closed".to_string();
    }
    if let Ok(value) = serde_json::from_slice::<Value>(payload) {
        if let Some(reason) = value.as_str() {
            return reason.to_string();
        }
        if let Some(reason) = value.get("reason").and_then(Value::as_str) {
            return reason.to_string();
        }
        if let Some(kind) = value.get("kind").and_then(Value::as_str) {
            return kind.to_string();
        }
    }
    String::from_utf8_lossy(payload).into_owned()
}

/// 程序自己结束（不再重来）还是连接/core 层面的断开（保留意图，重连后重新 attach）。
///
/// 实测两种 payload：
/// - 程序退出：`{"exit_code":0,"reason":"exited","signal":null}`
/// - `hc server stop`：`{"exit_code":null,"reason":"server_stop","signal":1}`
///
/// 所以不能只看 `exit_code` 是否存在，要看 `reason`。
pub fn close_is_program_exit(payload: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<Value>(payload) else {
        return false;
    };
    match value.get("reason").and_then(Value::as_str) {
        // 这些是 core 侧的事件：重连后应当重新 attach（终端可能已经不存在）。
        Some("server_stop" | "shutdown" | "restart" | "core_stop" | "lost") => false,
        Some(_) => true,
        None => value
            .get("exit_code")
            .map(|code| !code.is_null())
            .unwrap_or(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_real_subscribe_result() {
        let value = serde_json::json!({
            "seq": 1,
            "channel": 1,
            "terminals": [{
                "address": "t1", "state": "live", "title": "wxb24@omarchy:/tmp",
                "name": null, "cols": 200, "rows": 120,
                "labels": {}, "argv": ["sh"], "cwd": "/tmp", "created_ms": 1, "screen": "exact"
            }]
        });
        let result = SubscribeResult::parse(&value).unwrap();
        assert_eq!(result.seq, 1);
        assert_eq!(result.channel, Some(1));
        assert_eq!(result.terminals.len(), 1);
        assert_eq!(result.terminals[0].address, "t1");
        assert_eq!(result.terminals[0].label(), "wxb24@omarchy:/tmp");
        assert!(result.terminals[0].live);
        assert_eq!(result.terminals[0].status, None);
    }

    #[test]
    fn parses_an_attach_result() {
        let value = serde_json::json!({
            "channel": 2, "resize_notifications": false,
            "terminal": {"address": "t1", "state": "live", "title": "", "cols": 80, "rows": 24}
        });
        let result = AttachResult::parse(&value).unwrap();
        assert_eq!(result.channel, 2);
        assert!(!result.resize_notifications);
        assert_eq!(result.terminal.unwrap().address, "t1");
    }

    #[test]
    fn parses_an_event_line() {
        let params = serde_json::json!({
            "type": "size", "terminal": "t1", "seq": 2,
            "data": {"cols": 90, "rows": 20}
        });
        let event = Event::parse(&params).unwrap();
        assert_eq!(event.kind, "size");
        assert_eq!(event.terminal.as_deref(), Some("t1"));
        assert_eq!(event.seq, 2);
        assert_eq!(event.data["cols"], 90);
        assert!(!event.is_resync());
    }

    #[test]
    fn a_status_object_yields_its_state_word() {
        let value = serde_json::json!({
            "address": "t1", "state": "live",
            "status": {"state": "blocked", "kind": "permission", "msg": "allow?"}
        });
        assert_eq!(
            TerminalSummary::parse(&value).unwrap().status.as_deref(),
            Some("blocked")
        );
        let legacy = serde_json::json!({"address": "t1", "state": "live", "status": "idle"});
        assert_eq!(
            TerminalSummary::parse(&legacy).unwrap().status.as_deref(),
            Some("idle")
        );
    }

    #[test]
    fn finished_terminals_are_not_live() {
        let value = serde_json::json!({"address": "t4", "state": "finished", "title": "vim"});
        let terminal = TerminalSummary::parse(&value).unwrap();
        assert!(!terminal.live);
        assert_eq!(terminal.label(), "vim");
    }

    #[test]
    fn program_exit_is_distinguishable_from_a_lost_connection() {
        let exited = br#"{"exit_code":0,"finished_ms":1,"reason":"exited","signal":null}"#;
        assert_eq!(close_reason(exited), "exited");
        assert!(close_is_program_exit(exited));

        // hc server stop：exit_code 是 null，signal 有值，reason 是 server_stop。
        let stopped = br#"{"exit_code":null,"finished_ms":1,"reason":"server_stop","signal":1}"#;
        assert_eq!(close_reason(stopped), "server_stop");
        assert!(!close_is_program_exit(stopped));

        // 被信号杀掉的程序仍然是「程序结束」。
        let signaled = br#"{"exit_code":null,"reason":"signaled","signal":9}"#;
        assert!(close_is_program_exit(signaled));

        assert!(!close_is_program_exit(br#"{"reason":"lost"}"#));
        assert!(!close_is_program_exit(b""));
    }

    #[test]
    fn close_reason_understands_json_and_text() {
        assert_eq!(close_reason(br#"{"reason":"exit"}"#), "exit");
        assert_eq!(close_reason(br#"{"kind":"finished"}"#), "finished");
        assert_eq!(close_reason(b"eof"), "eof");
        assert_eq!(close_reason(b""), "closed");
    }
}
