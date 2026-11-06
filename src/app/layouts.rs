//! Personal TUI layout profiles. These files contain data, never running programs.
use super::*;
use std::io::Write;

impl App {
    fn profile_path(&self, kind: &str, name: &str) -> Result<std::path::PathBuf> {
        anyhow::ensure!(
            !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "profile names use letters, digits, - or _"
        );
        Ok(self
            .config
            .config_path
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .join(kind)
            .join(format!("{name}.json")))
    }
    pub(super) fn clone_layout(&mut self, name: &str) -> Result<()> {
        self.assign_origins();
        let wi = self.active_ws;
        let ti = self
            .workspace()
            .ok_or_else(|| anyhow::anyhow!("no space"))?
            .active;
        let ws = &self.workspaces[wi];
        let machine = ws.machine.as_deref().unwrap_or("");
        let raw = crate::shared::encode(&self.snapshot(), machine, &json!({"spaces":[]}));
        let space_id = ws
            .origin
            .as_deref()
            .unwrap_or("")
            .strip_prefix(&format!("{machine}/"))
            .unwrap_or(ws.origin.as_deref().unwrap_or(""));
        let space = raw["spaces"]
            .as_array()
            .and_then(|a| a.iter().find(|w| w["id"] == space_id))
            .ok_or_else(|| {
                anyhow::anyhow!("layout contains a terminal that is not attached yet")
            })?;
        let root = space["tabs"][ti]["root"].clone();
        anyhow::ensure!(root.is_object(), "layout not ready");
        let path = self.profile_path("layouts", name)?;
        std::fs::create_dir_all(path.parent().unwrap())?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        writeln!(file, "{}", serde_json::to_string_pretty(&root)?)?;
        self.note = format!("Saved layout {name}");
        Ok(())
    }
    pub(super) fn load_layout(&mut self, name: &str) -> Result<()> {
        if let Some(index) = crate::layout::preset::NAMES.iter().position(|n| *n == name) {
            let tab = self
                .workspaces
                .get_mut(self.active_ws)
                .and_then(|w| w.tab_mut())
                .ok_or_else(|| anyhow::anyhow!("no tab"))?;
            tab.tree = crate::layout::preset::build(index, &tab.tree.panes());
            tab.layout = Some(index);
            self.mark_layout();
            return Ok(());
        }
        let root = serde_json::from_str(&std::fs::read_to_string(
            self.profile_path("layouts", name)?,
        )?)?;
        self.apply_layout(root)
    }
    pub(super) fn apply_layout(&mut self, root: Value) -> Result<()> {
        let wi = self.active_ws;
        let ti = self
            .workspace()
            .ok_or_else(|| anyhow::anyhow!("no space"))?
            .active;
        if let Ok(tree) = serde_json::from_value::<SavedNode>(root.clone()) {
            let ids = tree.panes();
            let unique: std::collections::HashSet<_> = ids.iter().copied().collect();
            let owned: std::collections::HashSet<_> = self.workspaces[wi].tabs[ti]
                .panes()
                .iter()
                .map(|p| p.0)
                .collect();
            anyhow::ensure!(
                !ids.is_empty() && ids.len() == unique.len() && unique.is_subset(&owned),
                "layout must use each existing tab pane at most once"
            );
            for id in owned.difference(&unique) {
                self.remove_pane_view(*id);
            }
            let tab = &mut self.workspaces[wi].tabs[ti];
            tab.floating.clear();
            tab.tree = (&tree).into();
            if !unique.contains(&tab.focus) {
                tab.focus = ids[0];
            }
            self.mark_layout();
            return Ok(());
        }
        self.assign_origins();
        let ws = &self.workspaces[wi];
        let tab = &ws.tabs[ti];
        let machine = ws.machine.as_deref().unwrap_or("");
        let raw_id = |id: &str| {
            id.strip_prefix(&format!("{machine}/"))
                .unwrap_or(id)
                .to_string()
        };
        let raw = json!({"spaces":[{"id":raw_id(ws.origin.as_deref().unwrap()),"name":ws.label,"folder":ws.folder,"tabs":[{"id":raw_id(tab.origin.as_deref().unwrap()),"name":tab.label,"root":root}]}]});
        let mut imported = crate::shared::decode(&raw, machine).map_err(anyhow::Error::msg)?;
        anyhow::ensure!(
            imported.workspaces[0].tabs.len() == 1,
            "layout needs at least one content block"
        );
        let mut saved = self.snapshot();
        let mut replacement = imported.workspaces.remove(0).tabs.remove(0);
        let mut next = saved
            .panes
            .keys()
            .filter_map(|s| s.parse::<u32>().ok())
            .max()
            .unwrap_or(0)
            + 1;
        let mut mapping = BTreeMap::new();
        renumber_node(&mut replacement.tree, &mut mapping, &mut next);
        replacement.focus = replacement.tree.panes()[0];
        for (old, new) in mapping {
            if let Some(pane) = imported.panes.remove(&old.to_string()) {
                saved.panes.insert(new.to_string(), pane);
            }
        }
        saved.workspaces[wi].tabs[ti] = replacement;
        self.restore(saved);
        self.mark_layout();
        Ok(())
    }
    pub(super) fn clone_panel(&mut self, name: &str, target: &str) -> Result<()> {
        let panel = self
            .config
            .panels
            .iter()
            .find(|p| p.name == name)
            .cloned()
            .or_else(|| {
                crate::app::panels::builtin(name).map(|command| crate::config::PanelDefinition {
                    name: name.to_string(),
                    command: command.to_string(),
                    refresh: 5,
                })
            })
            .ok_or_else(|| anyhow::anyhow!("unknown panel {name}"))?;
        let mut panel = panel;
        panel.name = target.to_string();
        let path = self.profile_path("panels", target)?;
        std::fs::create_dir_all(path.parent().unwrap())?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        writeln!(file, "{}", serde_json::to_string_pretty(&panel)?)?;
        self.config.panels.push(panel);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn app() -> (App, std::path::PathBuf) {
        let root = std::env::temp_dir().join(crate::shared::identity("hctui-profiles-"));
        std::fs::create_dir_all(&root).unwrap();
        let (_, events) = std::sync::mpsc::channel();
        let app = App::with_network(
            Config {
                restore: false,
                config_path: root.join("config.toml"),
                state_path: root.join("state.json"),
                ..Config::default()
            },
            None,
            events,
        )
        .unwrap();
        (app, root)
    }
    #[test]
    fn layout_and_panel_clones_roundtrip_without_overwriting_files() {
        let (mut app, root) = app();
        let id = app.focused();
        app.panes.get_mut(&id).unwrap().terminal = Some("t1".into());
        app.clone_layout("personal").unwrap();
        assert!(app.clone_layout("personal").is_err());
        app.load_layout("personal").unwrap();
        assert_eq!(app.focused_pane().unwrap().terminal.as_deref(), Some("t1"));
        app.clone_panel("files", "my-files").unwrap();
        assert!(app.clone_panel("files", "my-files").is_err());
        let config = Config::load(Some(root.join("config.toml")));
        assert!(config.panels.iter().any(|p| p.name == "my-files"));
        assert!(app.clone_layout("../outside").is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn invalid_layout_keeps_current_tree() {
        let (mut app, root) = app();
        let before = app.tab().unwrap().tree.clone();
        assert!(app
            .apply_layout(json!({"kind":"leaf","pane":99999}))
            .is_err());
        assert_eq!(app.tab().unwrap().tree, before);
        std::fs::remove_dir_all(root).unwrap();
    }
}
