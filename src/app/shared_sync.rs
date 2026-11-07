use super::*;
use crate::i18n::{t, t1, t2};
use crate::shared;

#[derive(Clone)]
pub(super) struct Replica {
    pub base: Value,
    pub version: u64,
    pub pending: Option<Value>,
    pub blocked: bool,
    pub needs_refresh: bool,
}

impl App {
    pub(super) fn request_shared(&mut self, machine: &str) {
        if self.inflight.contains_key(&format!("shared.get:{machine}")) {
            return;
        }
        self.call_rpc(
            &format!("shared.get:{machine}"),
            "doc.get",
            json!({"name":shared::DOCUMENT,"on": if machine.is_empty(){None}else{Some(machine)}}),
        );
    }

    pub(super) fn assign_origins(&mut self) {
        for workspace in &mut self.workspaces {
            let machine = workspace.machine.as_deref().unwrap_or("");
            let qualify = |id: String| {
                if machine.is_empty() {
                    id
                } else {
                    format!("{machine}/{id}")
                }
            };
            workspace
                .origin
                .get_or_insert_with(|| qualify(shared::identity("sp")));
            for tab in &mut workspace.tabs {
                tab.origin
                    .get_or_insert_with(|| qualify(shared::identity("tb")));
                for id in tab.panes() {
                    if let Some(pane) = self.panes.get_mut(&id.0) {
                        pane.origin
                            .get_or_insert_with(|| qualify(shared::identity("p")));
                    }
                }
            }
        }
    }

    pub(super) fn shared_reply(
        &mut self,
        tag: &str,
        result: std::result::Result<Value, crate::hc::net::RpcFailure>,
    ) -> bool {
        if let Some(machine) = tag.strip_prefix("shared.get:") {
            match result {
                Ok(reply) => {
                    let value = reply
                        .get("value")
                        .cloned()
                        .filter(|v| !v.is_null())
                        .unwrap_or_else(|| json!({"spaces":[]}));
                    let version = reply.get("version").and_then(Value::as_u64).unwrap_or(0);
                    if self.inflight.contains_key(&format!("shared.set:{machine}")) {
                        if let Some(replica) = self.shared_docs.get_mut(machine) {
                            replica.needs_refresh = true;
                        }
                        return true;
                    }
                    if let Some(previous) = self.shared_docs.get(machine).cloned() {
                        if previous.blocked {
                            if let Some(replica) = self.shared_docs.get_mut(machine) {
                                replica.base = value;
                                replica.version = version;
                            }
                            return true;
                        }
                        let desired = previous.pending.or_else(|| {
                            if !self.shared_dirty {
                                return None;
                            }
                            let value = shared::encode(&self.snapshot(), machine, &previous.base);
                            (value != previous.base).then_some(value)
                        });
                        if let Some(pending) = desired {
                            match shared::merge(&previous.base, &pending, &value) {
                                Ok(merged) => {
                                    if let Err(error) = self.import_shared(&merged, machine) {
                                        self.note = t1("Cannot rebase layout: {}", error);
                                        return true;
                                    }
                                    self.shared_docs.insert(
                                        machine.to_string(),
                                        Replica {
                                            base: value,
                                            version,
                                            pending: Some(merged),
                                            blocked: false,
                                            needs_refresh: false,
                                        },
                                    );
                                    self.shared_dirty = true;
                                    self.flush_shared();
                                }
                                Err(error) => {
                                    self.note = t1(
                                            "Shared layout: {}; use layout.reload to accept the shared version",
                                            error,
                                        );
                                    self.shared_docs.insert(
                                        machine.to_string(),
                                        Replica {
                                            base: value,
                                            version,
                                            pending: None,
                                            blocked: true,
                                            needs_refresh: false,
                                        },
                                    );
                                }
                            }
                            return true;
                        }
                        if previous.version == version {
                            return true;
                        }
                    }
                    if let Err(error) = self.import_shared(&value, machine) {
                        self.note = t1("Cannot import layout: {}", error);
                        return true;
                    }
                    self.shared_docs.insert(
                        machine.to_string(),
                        Replica {
                            base: value,
                            version,
                            pending: None,
                            blocked: false,
                            needs_refresh: false,
                        },
                    );
                }
                Err(error) => {
                    let label = if machine.is_empty() {
                        t("Local")
                    } else {
                        machine
                    };
                    self.note = t2("{} layout unavailable: {}", label, error.message)
                }
            }
            self.dirty = true;
            return true;
        }
        if let Some(machine) = tag.strip_prefix("shared.set:") {
            match result {
                Ok(reply) => {
                    if let Some(replica) = self.shared_docs.get_mut(machine) {
                        replica.version = reply
                            .get("version")
                            .and_then(Value::as_u64)
                            .unwrap_or(replica.version + 1);
                        if let Some(pending) = replica.pending.take() {
                            replica.base = pending;
                        }
                        self.shared_dirty = true;
                    }
                    if self
                        .shared_docs
                        .get(machine)
                        .is_some_and(|r| r.needs_refresh)
                    {
                        self.shared_docs.get_mut(machine).unwrap().needs_refresh = false;
                        self.request_shared(machine);
                    }
                }
                Err(error) if error.kind == "conflict" => self.request_shared(machine),
                Err(error) => {
                    self.note = t1("Shared layout write failed: {}", error.message);
                }
            }
            self.dirty = true;
            return true;
        }
        false
    }

    pub(super) fn flush_shared(&mut self) {
        if !self.config.restore || !self.shared_dirty {
            return;
        }
        if self
            .panes
            .values()
            .any(|p| !p.transient && p.module.is_none() && p.terminal.is_none())
        {
            return;
        }
        self.assign_origins();
        let snapshot = self.snapshot();
        let machines: Vec<_> = self.shared_docs.keys().cloned().collect();
        for machine in machines {
            let tag = format!("shared.set:{machine}");
            if self.inflight.contains_key(&tag) {
                continue;
            }
            let replica = self.shared_docs.get(&machine).unwrap();
            if replica.blocked {
                continue;
            }
            let value = replica
                .pending
                .clone()
                .unwrap_or_else(|| shared::encode(&snapshot, &machine, &replica.base));
            if value == replica.base {
                continue;
            }
            let version = replica.version;
            self.shared_docs.get_mut(&machine).unwrap().pending = Some(value.clone());
            self.call_rpc(&tag,"doc.set",json!({"name":shared::DOCUMENT,"on":if machine.is_empty(){None}else{Some(&machine)},"value":value,"version":version}));
        }
        self.shared_dirty = false;
    }

    fn import_shared(&mut self, value: &Value, machine: &str) -> Result<(), String> {
        let mut imported = shared::decode(value, machine)?;
        let mut current = self.snapshot();
        let current_workspace = self.workspace().and_then(|w| w.origin.clone());
        let focused_terminal = self.focused_pane().and_then(|p| p.terminal.clone());
        let imported_terminals: std::collections::HashSet<_> = imported
            .panes
            .values()
            .filter_map(|p| p.terminal.clone())
            .collect();
        let imported_ids: std::collections::HashSet<_> = imported
            .workspaces
            .iter()
            .filter_map(|w| w.origin.clone())
            .collect();
        // Preserve local-only work and remove duplicate views of imported terminals.
        for ws in &mut current.workspaces {
            if ws.machine.as_deref().unwrap_or("") == machine
                && (ws
                    .origin
                    .as_ref()
                    .is_some_and(|id| imported_ids.contains(id))
                    || self.shared_docs.get(machine).is_some_and(|r| {
                        r.base["spaces"].as_array().is_some_and(|a| {
                            a.iter().any(|w| {
                                ws.origin.as_deref().is_some_and(|id| {
                                    id == qualify(machine, w["id"].as_str().unwrap_or(""))
                                })
                            })
                        })
                    }))
            {
                ws.tabs.clear();
                continue;
            }
            for tab in &mut ws.tabs {
                let mut tree: SplitNode = (&tab.tree).into();
                let mut empty = false;
                for id in tab.tree.panes() {
                    if current
                        .panes
                        .get(&id.to_string())
                        .and_then(|p| p.terminal.as_ref())
                        .is_some_and(|t| imported_terminals.contains(t))
                    {
                        match tree.clone().remove_pane(PaneId(id)) {
                            Some(next) => tree = next,
                            None => {
                                empty = true;
                                break;
                            }
                        }
                    }
                }
                if empty {
                    tab.tree = SavedNode::Stack {
                        panes: Vec::new(),
                        active: 0,
                    };
                } else {
                    tab.tree = SavedNode::from(&tree);
                }
                tab.floating.retain(|f| {
                    !current
                        .panes
                        .get(&f.pane.to_string())
                        .and_then(|p| p.terminal.as_ref())
                        .is_some_and(|t| imported_terminals.contains(t))
                });
            }
            ws.tabs.retain(|t| !t.tree.panes().is_empty());
        }
        current.workspaces.retain(|w| !w.tabs.is_empty());
        let mut next = current
            .panes
            .keys()
            .filter_map(|s| s.parse::<u32>().ok())
            .max()
            .unwrap_or(0)
            + 1;
        let mut mapping = BTreeMap::new();
        for ws in &mut imported.workspaces {
            if let Some(local) = self.workspaces.iter().find(|w| w.origin == ws.origin) {
                let active = local.tab().and_then(|t| t.origin.as_ref());
                if let Some(index) = ws.tabs.iter().position(|t| t.origin.as_ref() == active) {
                    ws.active_tab = index;
                }
            }
            for tab in &mut ws.tabs {
                let old_origin = tab.origin.clone();
                let local = self
                    .workspaces
                    .iter()
                    .flat_map(|w| &w.tabs)
                    .find(|t| t.origin == old_origin);
                tab.settled = local.is_some_and(|t| t.settled);
                let local_focus = local.and_then(|t| self.panes.get(&t.focus));
                let focused = tab.tree.panes().into_iter().find(|id| {
                    imported.panes.get(&id.to_string()).is_some_and(|p| {
                        local_focus.is_some_and(|local| {
                            (p.origin.is_some() && p.origin == local.origin)
                                || (p.terminal.is_some() && p.terminal == local.terminal)
                        })
                    })
                });
                renumber_node(&mut tab.tree, &mut mapping, &mut next);
                tab.focus = focused
                    .and_then(|id| mapping.get(&id).copied())
                    .or_else(|| tab.tree.panes().first().copied())
                    .unwrap_or_default();
            }
        }
        for (old, new) in mapping {
            if let Some(p) = imported.panes.remove(&old.to_string()) {
                current.panes.insert(new.to_string(), p);
            }
        }
        current.workspaces.extend(imported.workspaces);
        if current.workspaces.is_empty() {
            for pane in self.panes.keys().copied().collect::<Vec<_>>() {
                self.remove_pane_view(pane);
            }
            self.workspaces.clear();
            self.panes.clear();
            self.dirty = true;
            return Ok(());
        }
        current.active_workspace = current_workspace
            .and_then(|id| {
                current
                    .workspaces
                    .iter()
                    .position(|w| w.origin.as_deref() == Some(&id))
            })
            .unwrap_or(0);
        self.restore(current);
        if let Some(pane) = focused_terminal.and_then(|t| self.pane_by_terminal(&t)) {
            self.focus_pane(pane);
        }
        Ok(())
    }
}
fn qualify(machine: &str, id: &str) -> String {
    if machine.is_empty() {
        id.to_string()
    } else {
        format!("{machine}/{id}")
    }
}

impl App {
    pub(super) fn refresh_shared_labels(&mut self) {
        if !self.config.restore {
            return;
        }
        let mut updates = Vec::new();
        for (wi, workspace) in self.workspaces.iter().enumerate() {
            let machine = workspace.machine.as_deref().unwrap_or("");
            if !self.shared_docs.get(machine).is_some_and(|r| !r.blocked) {
                continue;
            }
            let local = |id: &str| {
                id.strip_prefix(&format!("{machine}/"))
                    .unwrap_or(id)
                    .to_string()
            };
            let Some(space) = workspace.origin.as_deref().map(local) else {
                continue;
            };
            for (ti, tab) in workspace.tabs.iter().enumerate() {
                let Some(tab_id) = tab.origin.as_deref().map(local) else {
                    continue;
                };
                let where_label = format!(
                    "{} / {}",
                    self.workspace_label(wi),
                    if tab.label.is_empty() {
                        (ti + 1).to_string()
                    } else {
                        tab.label.clone()
                    }
                );
                for id in tab.panes() {
                    let Some(pane) = self.panes.get(&id.0) else {
                        continue;
                    };
                    if pane.transient {
                        continue;
                    }
                    let address = self
                        .shown_original
                        .get(&id.0)
                        .cloned()
                        .unwrap_or_else(|| pane.terminal.clone());
                    let Some(address) = address else {
                        continue;
                    };
                    let value = (space.clone(), tab_id.clone(), where_label.clone());
                    if self.shared_labels.get(&address) != Some(&value) {
                        updates.push((address, value));
                    }
                }
            }
        }
        for (address, value) in updates {
            self.call_rpc(&format!("where:{address}"),"terminal.label",json!({"terminal":address,"labels":{"herdr.space":value.0,"herdr.tab":value.1,"where":value.2}}));
            self.shared_labels.insert(address, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn space(id: &str, name: &str, terminal: &str) -> Value {
        json!({"id":id,"name":name,"folder":"/repo","tabs":[{"id":format!("tb{id}"),"name":"","root":{"type":"row","children":[{"id":format!("p{id}"),"type":"block","content":{"terminal":terminal}}]}}]})
    }
    #[test]
    fn live_layout_updates_preserve_pane_identity_stream_and_screen() {
        let mut app = app();
        let base = json!({"spaces":[space("a","A","t1")]});
        app.shared_reply("shared.get:", Ok(json!({"version":1,"value":base})));
        let id = app.pane_by_terminal("t1").unwrap();
        let pane = app.panes.get_mut(&id).unwrap();
        pane.attached(17, 80, 24);
        pane.mark_spawned((80, 24));
        pane.view.write(b"retained screen");
        let old_ws = app.workspaces[0].id;
        let changed = json!({"spaces":[space("a","Renamed","t1")]});
        app.shared_reply("shared.get:", Ok(json!({"version":2,"value":changed})));
        assert_eq!(app.pane_by_terminal("t1"), Some(id));
        assert_eq!(app.workspaces[0].id, old_ws);
        assert_eq!(app.panes[&id].channel(), Some(17));
        assert!(!app.panes[&id].needs_spawn());
        assert!(app.panes[&id]
            .view
            .viewport_lines()
            .join("\n")
            .contains("retained screen"));
    }
    #[test]
    fn gui_update_merges_local_changes_before_the_debounce_write() {
        let mut app = app();
        let base = json!({"spaces":[space("a","A","t1"),space("b","B","t2")]});
        app.shared_reply("shared.get:", Ok(json!({"version":1,"value":base})));
        app.config.restore = true;
        app.workspaces[0].label = "local pending".into();
        app.mark_layout();
        let mut remote = base;
        remote["spaces"][1]["name"] = json!("remote pending");
        app.shared_reply("shared.get:", Ok(json!({"version":2,"value":remote})));
        assert_eq!(app.workspaces[0].label, "local pending");
        assert_eq!(app.workspaces[1].label, "remote pending");
    }
    #[test]
    fn same_named_agent_groups_fold_by_source_id_not_by_label() {
        let mut app = app();
        let value = json!({"spaces":[space("a","same","t1"),space("b","same","t2")]});
        app.shared_reply("shared.get:", Ok(json!({"version":1,"value":value})));
        for pane in app.panes.values_mut() {
            pane.agent.set_status(Some("working".into()));
        }
        let rows = app.navigation_rows();
        assert_ne!(rows[0].group_id, rows[1].group_id);
        assert_eq!(rows[0].group, rows[1].group);
    }
    #[test]
    fn conflicting_edits_block_automatic_overwrite_until_reload() {
        let mut app = app();
        let base = json!({"spaces":[space("a","A","t1")]});
        app.shared_reply("shared.get:", Ok(json!({"version":1,"value":base})));
        app.config.restore = true;
        app.workspaces[0].label = "ours".into();
        app.mark_layout();
        app.flush_shared();
        app.inflight.remove("shared.set:");
        let mut theirs = base;
        theirs["spaces"][0]["name"] = json!("theirs");
        app.shared_reply("shared.get:", Ok(json!({"version":2,"value":theirs})));
        assert!(app.shared_docs[""].blocked);
        // 冲突提示指出冲突路径并给出恢复动作。
        assert!(app.note.contains("Shared layout:"), "{}", app.note);
        assert!(app.note.contains("layout.reload"), "{}", app.note);
        app.mark_layout();
        app.flush_shared();
        assert!(!app.inflight.contains_key("shared.set:"));
        // layout.reload 解除阻塞并重新取共享版本。
        app.execute_action("layout.reload", json!({})).unwrap();
        assert!(!app.shared_docs[""].blocked);
        assert!(app.inflight.contains_key("shared.get:"));
    }
    #[test]
    fn same_named_spaces_have_distinct_stable_identities_and_remote_addresses() {
        let mut app = app();
        let value = json!({"spaces":[space("a","same","t1"),space("b","same","t2")]});
        app.shared_reply("shared.get:vps", Ok(json!({"version":1,"value":value})));
        assert_eq!(app.workspaces.len(), 2);
        assert_eq!(app.workspaces[0].origin.as_deref(), Some("vps/a"));
        assert_eq!(app.workspaces[1].origin.as_deref(), Some("vps/b"));
        assert!(app.pane_by_terminal("vps/t1").is_some());
        assert_eq!(app.workspace_machine(0), "vps");
        let saved = app.snapshot();
        app.restore(saved);
        assert_eq!(app.workspaces[1].origin.as_deref(), Some("vps/b"));
    }
    #[test]
    fn shared_removal_and_move_update_the_model_without_killing_terminals() {
        let mut app = app();
        app.shared_reply(
            "shared.get:",
            Ok(json!({"version":1,"value":{"spaces":[space("a","A","t1"),space("b","B","t2")]}})),
        );
        let id = app.pane_by_terminal("t2").unwrap();
        app.focus_pane(id);
        app.shared_reply(
            "shared.get:",
            Ok(json!({"version":2,"value":{"spaces":[space("b","renamed","t2")]}})),
        );
        assert_eq!(app.workspaces.len(), 1);
        assert_eq!(app.workspaces[0].label, "renamed");
        assert_eq!(app.focused_pane().unwrap().terminal.as_deref(), Some("t2"));
        assert!(!app
            .inflight
            .values()
            .any(|(method, _)| method == "terminal.kill"));
    }
    #[test]
    fn concurrent_other_space_edits_are_rebased_before_cas_retry() {
        let mut app = app();
        let base = json!({"spaces":[space("a","A","t1"),space("b","B","t2")]});
        app.shared_reply("shared.get:", Ok(json!({"version":1,"value":base})));
        app.config.restore = true;
        app.workspaces[0].label = "ours".into();
        app.mark_layout();
        app.flush_shared();
        assert!(app.shared_docs[""].pending.is_some());
        app.inflight.remove("shared.set:");
        app.shared_reply(
            "shared.set:",
            Err(crate::hc::net::RpcFailure {
                kind: "conflict".into(),
                message: "CAS conflict".into(),
            }),
        );
        let mut theirs = base.clone();
        theirs["spaces"][1]["name"] = json!("theirs");
        app.shared_reply("shared.get:", Ok(json!({"version":2,"value":theirs})));
        let pending = app.shared_docs[""].pending.as_ref().unwrap();
        assert_eq!(pending["spaces"][0]["name"], "ours");
        assert_eq!(pending["spaces"][1]["name"], "theirs");
        assert_eq!(app.inflight["shared.set:"].1["version"], 2);
        assert_eq!(app.workspaces[1].label, "theirs");
        app.inflight.remove("shared.set:");
        app.shared_reply("shared.set:", Ok(json!({"version":3})));
        app.flush_shared();
        assert!(!app.inflight.contains_key("shared.set:"));
    }
}
