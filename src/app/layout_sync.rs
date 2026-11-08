//! `hctui-layout-v1` 的写入路径：与 `herdr-app` 同一条链路——
//! 读 → 三方合并 → 带版本 CAS 写 → 冲突重读重放 → 合并失败阻塞自动覆盖。
//!
//! 这里是唯一构造并提交布局文档写入的地方；`src/app.rs` 里的
//! `doc.get` / `doc.sync` / `doc.set` 回复分支只做转发。
//!
//! 文档里的 pane/tab/workspace 编号是写入方进程内的编号，换一个写入者就对
//! 不上。`base` 保存「最近一次接受并导入的本地快照」，远端值先经
//! [`shared::translate`] 翻译进这套编号，三方合并才在同一个编号空间里比较。

use super::*;
use crate::i18n::{t, t1};
use crate::shared;

/// 把文档值解析成可用的布局；缺失或不可用时返回 `None`。
fn layout_state(value: &Value) -> Option<SavedState> {
    if value.is_null() {
        return None;
    }
    let state = match value {
        Value::String(text) => SavedState::parse(text),
        other => SavedState::parse(&other.to_string()),
    }?;
    state.is_usable().then_some(state)
}

impl App {
    /// 读布局文档；`tag` 决定回复走哪个分支（首次加载或冲突重读）。
    pub(super) fn request_layout(&mut self, tag: &str) {
        if self.inflight.contains_key(tag) {
            return;
        }
        self.call_rpc(tag, "doc.get", json!({ "name": DOC_NAME }));
    }

    /// `doc.get` / `doc.sync` / `doc.set` 的回复；返回 `false` 表示不是布局文档的。
    pub(super) fn layout_reply(
        &mut self,
        tag: &str,
        result: std::result::Result<Value, crate::hc::net::RpcFailure>,
    ) -> bool {
        match tag {
            "doc.get" => self.layout_read(true, result),
            "doc.sync" => self.layout_read(false, result),
            "doc.set" => self.layout_written(result),
            _ => return false,
        }
        self.dirty = true;
        true
    }

    fn snapshot_value(&self) -> Value {
        serde_json::to_value(self.snapshot()).unwrap_or(Value::Null)
    }

    /// 接受远端版本：整份导入，`base` 换成导入后的本地快照。
    fn adopt_layout(&mut self, value: &Value, version: u64) {
        match layout_state(value) {
            Some(state) => {
                self.restore(state);
                self.layout.base = self.snapshot_value();
            }
            None => self.layout.base = Value::Null,
        }
        self.layout.version = version;
        self.layout.pending = None;
        self.layout.blocked = false;
        self.layout.needs_refresh = false;
        self.layout_dirty = false;
        self.layout_since = None;
    }

    fn block_layout(&mut self, reason: String) {
        self.note = t1(
            "Layout: {}; use layout.reload to accept the shared version",
            reason,
        );
        self.layout.blocked = true;
        self.layout.pending = None;
        self.layout_dirty = false;
        self.layout_since = None;
    }

    fn layout_read(
        &mut self,
        initial: bool,
        result: std::result::Result<Value, crate::hc::net::RpcFailure>,
    ) {
        let reply = match result {
            Ok(reply) => reply,
            Err(error) => {
                if initial {
                    self.doc_loaded = false;
                    self.restore_pending = false;
                    self.ensure_layout();
                    self.note = t1("cannot read layout: {}", error.message);
                } else {
                    self.note = t1("layout sync failed: {}", error.message);
                }
                return;
            }
        };
        let value = reply
            .get("value")
            .cloned()
            .filter(|value| !value.is_null())
            .unwrap_or(Value::Null);
        let version = reply.get("version").and_then(Value::as_u64).unwrap_or(0);
        self.doc_loaded = true;
        if self.inflight.contains_key("doc.set") {
            // 自己的写入还在飞：等它回来再按新版本合并。
            self.layout.needs_refresh = true;
            return;
        }
        if initial || self.layout.blocked {
            self.restore_pending = false;
            self.adopt_layout(&value, version);
            if initial {
                self.ensure_layout();
                // 初次读到文档就对账一次：认领活着的服务终端，回收声明没了的。
                self.services_reconcile(self.service_trigger);
                if self.config.restore {
                    self.request_shared("");
                    self.call_rpc("machines.sync", "machine.list", json!({}));
                }
            }
            return;
        }
        // 没有本地意图：远端版本变了就整份接受。
        if !self.layout_dirty {
            if self.layout.version != version {
                self.adopt_layout(&value, version);
            }
            return;
        }
        // 有本地意图：重编号后三方合并重放；合并不了就阻塞自动覆盖。
        self.assign_origins();
        let ours = self.snapshot_value();
        let merged = shared::translate(&value, &self.layout.base)
            .and_then(|theirs| {
                shared::merge(&self.layout.base, &ours, &theirs).map(|merged| (theirs, merged))
            })
            .and_then(|(theirs, merged)| match layout_state(&merged) {
                Some(state) => Ok((theirs, state)),
                None => Err(t("unsupported layout document").to_string()),
            });
        match merged {
            Ok((theirs, state)) => {
                self.restore(state);
                self.layout.base = theirs;
                self.layout.version = version;
                self.layout.pending = None;
                self.layout.blocked = false;
                self.layout_dirty = true;
                self.layout_since = None;
                self.flush_layout();
            }
            Err(error) => self.block_layout(error),
        }
    }

    fn layout_written(&mut self, result: std::result::Result<Value, crate::hc::net::RpcFailure>) {
        match result {
            Ok(reply) => {
                self.layout.version = reply
                    .get("version")
                    .and_then(Value::as_u64)
                    .unwrap_or(self.layout.version + 1);
                if let Some(pending) = self.layout.pending.take() {
                    self.layout.base = pending;
                }
                // 写入期间又改了布局：保持脏标记，让下一次防抖继续写。
                self.assign_origins();
                self.layout_dirty = self.snapshot_value() != self.layout.base;
                self.layout_since = self.layout_dirty.then(Instant::now);
                if self.layout.needs_refresh {
                    self.layout.needs_refresh = false;
                    self.request_layout("doc.sync");
                }
            }
            Err(error) if error.kind == "conflict" => {
                // 另一个写入者先到：本地意图还在内存里，重读远端值后合并重放。
                self.layout.pending = None;
                self.layout_dirty = true;
                self.request_layout("doc.sync");
            }
            Err(error) => {
                self.layout.pending = None;
                self.layout_dirty = false;
                self.layout_since = None;
                self.note = t1("layout not saved: {}", error.message);
            }
        }
    }

    /// 提交本地意图：带刚读到的版本 CAS 写本地快照。
    pub(super) fn flush_layout(&mut self) {
        if !self.doc_loaded || self.layout.blocked || !self.layout_dirty {
            return;
        }
        if ["doc.get", "doc.sync", "doc.set"]
            .iter()
            .any(|tag| self.inflight.contains_key(*tag))
        {
            return;
        }
        self.assign_origins();
        let value = self.snapshot_value();
        self.layout_dirty = false;
        self.layout_since = None;
        if value == self.layout.base {
            self.layout.pending = None;
            return;
        }
        let version = self.layout.version;
        self.layout.pending = Some(value.clone());
        self.call_rpc(
            "doc.set",
            "doc.set",
            json!({ "name": DOC_NAME, "value": value, "version": version }),
        );
    }

    /// 退出前是否还有没落地的文档写入。
    pub(super) fn writes_pending(&self) -> bool {
        self.layout.pending.is_some()
            || self.layout_dirty
            || self.layout.needs_refresh
            || self.shared_dirty
            || self
                .inflight
                .keys()
                .any(|tag| tag.starts_with("doc.") || tag.starts_with("shared."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 一份窗口写下的布局：一个 workspace、一个 tab、两个 pane，id 从 1 起。
    fn seed() -> Value {
        json!({
            "schema_version":3,"active_workspace":0,
            "workspaces":[{
                "id":1,"label":"","active_tab":0,
                "tabs":[{
                    "id":1,"label":"","focus":1,"settled":false,"floating":[],
                    "tree":{"kind":"split","axis":"horizontal","ratio":0.5,
                            "first":{"kind":"leaf","pane":1},"second":{"kind":"leaf","pane":2}}
                }]
            }],
            "panes":{
                "1":{"terminal":null,"module":null,"name":null,"origin":null},
                "2":{"terminal":null,"module":null,"name":null,"origin":null}
            }
        })
    }

    fn app() -> App {
        let (_, events) = std::sync::mpsc::channel();
        let mut app = App::with_network(
            Config {
                restore: false,
                state_path: "/nonexistent/hctui-state".into(),
                ..Config::default()
            },
            None,
            events,
        )
        .unwrap();
        app.workspaces.clear();
        app.panes.clear();
        app
    }

    #[test]
    fn a_second_writer_is_merged_into_the_layout_instead_of_overwritten() {
        let mut app = app();
        app.layout_reply("doc.get", Ok(json!({"version":1,"value":seed()})));
        app.inflight.remove("doc.get");
        assert!(app.doc_loaded);
        // 本地加一个 tab，然后写出去（CAS 到 version 1）。
        assert!(app.execute_action("tab.new", json!({})).is_ok());
        app.mark_layout();
        app.flush_layout();
        assert_eq!(app.inflight["doc.set"].1["version"], 1);
        // 另一个写入者先提交了：写回来的值只带一个 tab、另一个 label。
        app.inflight.remove("doc.set");
        app.layout_reply(
            "doc.set",
            Err(crate::hc::net::RpcFailure {
                kind: "conflict".into(),
                message: "CAS conflict".into(),
            }),
        );
        app.inflight.remove("doc.sync");
        let mut theirs = seed();
        theirs["workspaces"][0]["label"] = json!("EXTERNAL");
        theirs["workspaces"][0]["tabs"][0]["label"] = json!("ext");
        app.layout_reply("doc.sync", Ok(json!({"version":2,"value":theirs.clone()})));
        // 并集：外部写的 label 与本地多出来的 tab 都在，版本用刚读到的 2。
        let pending = &app.inflight["doc.set"].1;
        assert_eq!(pending["version"], 2);
        assert_eq!(pending["value"]["workspaces"][0]["label"], "EXTERNAL");
        assert_eq!(
            pending["value"]["workspaces"][0]["tabs"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn an_unmergeable_layout_blocks_the_write_until_reload() {
        let mut app = app();
        app.layout_reply("doc.get", Ok(json!({"version":1,"value":seed()})));
        app.inflight.remove("doc.get");
        assert!(app.execute_action("tab.new", json!({})).is_ok());
        app.mark_layout();
        app.flush_layout();
        app.inflight.remove("doc.set");
        // 同一个字段被两边改成不同的值：不猜，阻塞自动覆盖。
        app.workspaces[0].label = "ours".to_string();
        app.mark_layout();
        let mut theirs = seed();
        theirs["workspaces"][0]["label"] = json!("theirs");
        app.layout_reply("doc.sync", Ok(json!({"version":2,"value":theirs})));
        assert!(app.layout.blocked);
        assert!(app.note.contains("layout.reload"), "{}", app.note);
        app.mark_layout();
        app.flush_layout();
        assert!(!app.inflight.contains_key("doc.set"));
        // layout.reload 解除阻塞并重新取文档。
        app.execute_action("layout.reload", json!({})).unwrap();
        assert!(!app.layout.blocked);
        assert!(app.inflight.contains_key("doc.get"));
    }
}
