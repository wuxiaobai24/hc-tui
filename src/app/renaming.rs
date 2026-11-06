//! Terminal aliases are shared by Agent rows and every view of that terminal.
use super::*;

impl App {
    pub(super) fn terminal_alias(&self, address: &str) -> Result<String> {
        let pane = self
            .panes
            .values()
            .find(|pane| pane.terminal.as_deref() == Some(address));
        let terminal = self
            .terminals
            .iter()
            .find(|terminal| terminal.address == address);
        anyhow::ensure!(
            pane.is_some() || terminal.is_some(),
            t1("unknown terminal {}", address)
        );
        Ok(pane
            .and_then(|pane| pane.name.clone())
            .or_else(|| terminal.and_then(|terminal| terminal.name.clone()))
            .unwrap_or_default())
    }

    pub(super) fn rename_pane(&mut self, pane: u32, name: &str) -> Result<()> {
        let state = self
            .panes
            .get(&pane)
            .ok_or_else(|| anyhow::anyhow!(t("no pane")))?;
        if let Some(terminal) = state.terminal.clone() {
            self.rename_terminal(&terminal, name)?;
        } else {
            self.panes.get_mut(&pane).expect("pane exists").name = alias(name);
            self.mark_layout();
        }
        Ok(())
    }

    pub(super) fn rename_terminal(&mut self, address: &str, name: &str) -> Result<()> {
        self.terminal_alias(address)?;
        let name = name.trim();
        let alias = alias(name);
        for pane in self
            .panes
            .values_mut()
            .filter(|pane| pane.terminal.as_deref() == Some(address))
        {
            pane.name = alias.clone();
        }
        self.update_terminal(Some(address), |terminal| terminal.name = alias.clone());
        self.call_rpc(
            "label",
            "terminal.label",
            json!({"terminal": address, "labels": {"hctui": name}}),
        );
        self.mark_layout();
        Ok(())
    }
}

fn alias(name: &str) -> Option<String> {
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        let (_, events) = std::sync::mpsc::channel();
        App::with_network(
            Config {
                restore: false,
                prompt_new_tab: false,
                state_path: "/nonexistent/hctui-rename-test".into(),
                ..Config::default()
            },
            None,
            events,
        )
        .unwrap()
    }

    #[test]
    fn pane_and_agent_alias_updates_all_views_and_overrides_the_tab_label() {
        let mut app = app();
        let original = app.focused();
        app.panes.get_mut(&original).unwrap().terminal = Some("remote/t7".into());
        app.panes
            .get_mut(&original)
            .unwrap()
            .agent
            .set_status(Some("idle".into()));
        app.terminals.push(
            TerminalSummary::parse(
                &json!({"address":"remote/t7", "title":"shell", "status":"idle"}),
            )
            .unwrap(),
        );
        app.split_focused(Axis::Horizontal);
        let duplicate = app.focused();
        app.panes.get_mut(&duplicate).unwrap().terminal = Some("remote/t7".into());
        app.execute_action("tab.rename", json!({"name":"project"}))
            .unwrap();
        app.execute_action(
            "pane.rename",
            json!({"pane":original.to_string(), "name":"  审查 Agent  "}),
        )
        .unwrap();
        assert_eq!(app.panes[&original].name.as_deref(), Some("审查 Agent"));
        assert_eq!(app.panes[&duplicate].name.as_deref(), Some("审查 Agent"));
        assert_eq!(app.terminals[0].label(), "审查 Agent");
        assert!(app
            .navigation_rows()
            .iter()
            .any(|row| row.label.contains("审查 Agent")));
        assert_eq!(app.tab().unwrap().label, "project");
        assert_eq!(
            app.inflight["label"].1,
            json!({"terminal":"remote/t7", "labels":{"hctui":"审查 Agent"}})
        );
        app.execute_action("agent.rename", json!({"terminal":"remote/t7", "name":""}))
            .unwrap();
        assert!(app.panes[&original].name.is_none());
        assert!(app.panes[&duplicate].name.is_none());
        assert_eq!(app.terminals[0].label(), "shell");
        assert_eq!(app.inflight["label"].1["labels"]["hctui"], "");
    }

    #[test]
    fn external_agent_rename_keeps_the_original_address_when_focus_changes() {
        let mut app = app();
        let original = app.focused();
        app.terminals.push(
            TerminalSummary::parse(
                &json!({"address":"remote/t9", "labels":{"hctui":"old"}, "status":"idle"}),
            )
            .unwrap(),
        );
        app.execute_action("agent.rename", json!({"terminal":"remote/t9"}))
            .unwrap();
        assert_eq!(app.prompt.as_ref().unwrap().buffer, "old");
        let mut prompt = app.prompt.take().unwrap();
        prompt.buffer = "remote worker".into();
        app.new_tab(Some("elsewhere".into()));
        app.commit_prompt(prompt);
        assert_eq!(app.terminals[0].name.as_deref(), Some("remote worker"));
        assert_eq!(app.inflight["label"].1["terminal"], "remote/t9");
        assert!(app.panes[&original].name.is_none());
        assert!(app.pane_by_terminal("remote/t9").is_none());
    }

    #[test]
    fn tab_rename_prompt_survives_workspace_reordering() {
        let mut app = app();
        let workspace = app.workspace().unwrap().id;
        let tab = app.tab().unwrap().id;
        app.execute_action("tab.rename", json!({"name":"before"}))
            .unwrap();
        app.execute_action("tab.rename", json!({})).unwrap();
        assert_eq!(app.prompt.as_ref().unwrap().buffer, "before");
        let mut prompt = app.prompt.take().unwrap();
        prompt.buffer = "after".into();
        app.new_workspace(Some("other".into()));
        app.workspaces.swap(0, 1);
        app.commit_prompt(prompt);
        let workspace = app.workspaces.iter().find(|w| w.id == workspace).unwrap();
        assert_eq!(
            workspace.tabs.iter().find(|t| t.id == tab).unwrap().label,
            "after"
        );
        assert!(app
            .workspaces
            .iter()
            .filter(|w| w.id != workspace.id)
            .flat_map(|w| &w.tabs)
            .all(|t| t.label != "after"));
    }

    #[test]
    fn unattached_pane_rename_is_saved_and_unknown_targets_do_not_rename_focus() {
        let mut app = app();
        let pane = app.focused();
        app.execute_action("pane.rename", json!({"name":"local view"}))
            .unwrap();
        assert_eq!(
            app.snapshot().panes[&pane.to_string()].name.as_deref(),
            Some("local view")
        );
        app.execute_action("pane.rename", json!({})).unwrap();
        assert_eq!(app.prompt.as_ref().unwrap().buffer, "local view");
        assert!(app
            .execute_action(
                "agent.rename",
                json!({"terminal":"absent/t9", "name":"wrong"})
            )
            .is_err());
        assert_eq!(app.panes[&pane].name.as_deref(), Some("local view"));
    }
}
