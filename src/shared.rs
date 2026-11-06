//! The GUI's core document. Screen focus/zoom/settled state stays in this client.
use crate::i18n::t;
use crate::model::{SavedAxis, SavedNode, SavedPane, SavedState, SavedTab, SavedWorkspace};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

pub const DOCUMENT: &str = "herdr-app";

pub fn identity(prefix: &str) -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "{prefix}{time:x}{:x}{:x}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

fn qualify(machine: &str, id: &str) -> String {
    if machine.is_empty() || id.starts_with(&format!("{machine}/")) {
        id.to_string()
    } else {
        format!("{machine}/{id}")
    }
}
fn local<'a>(machine: &str, id: &'a str) -> &'a str {
    if machine.is_empty() {
        id
    } else {
        id.strip_prefix(machine)
            .and_then(|s| s.strip_prefix('/'))
            .unwrap_or(id)
    }
}

pub fn decode(value: &Value, machine: &str) -> Result<SavedState, String> {
    let mut state = SavedState::empty();
    let spaces = value
        .get("spaces")
        .and_then(Value::as_array)
        .ok_or(t("layout has no spaces array"))?;
    let mut next = 1;
    for space in spaces {
        let id = space
            .get("id")
            .and_then(Value::as_str)
            .ok_or(t("space has no id"))?;
        let mut ws = SavedWorkspace {
            origin: Some(qualify(machine, id)),
            machine: (!machine.is_empty()).then(|| machine.to_string()),
            folder: space
                .get("folder")
                .and_then(Value::as_str)
                .map(str::to_string),
            id: next,
            label: space
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            active_tab: 0,
            tabs: Vec::new(),
        };
        for tab in space
            .get("tabs")
            .and_then(Value::as_array)
            .ok_or(t("space has no tabs array"))?
        {
            let id = tab
                .get("id")
                .and_then(Value::as_str)
                .ok_or(t("tab has no id"))?;
            let Some(tree) = decode_node(
                tab.get("root").ok_or(t("tab has no root"))?,
                machine,
                &mut next,
                &mut state.panes,
            )?
            else {
                continue;
            };
            let focus = tree.panes().first().copied().unwrap_or_default();
            ws.tabs.push(SavedTab {
                origin: Some(qualify(machine, id)),
                settled: false,
                id: next,
                label: tab
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                focus,
                tree,
                floating: Vec::new(),
                layout: None,
            });
            next += 1;
        }
        state.workspaces.push(ws);
    }
    Ok(state)
}
fn decode_node(
    node: &Value,
    machine: &str,
    next: &mut u32,
    panes: &mut BTreeMap<String, SavedPane>,
) -> Result<Option<SavedNode>, String> {
    if let Some(children) = node.get("children").and_then(Value::as_array) {
        let mut parts = Vec::new();
        for child in children {
            if let Some(tree) = decode_node(child, machine, next, panes)? {
                let weight = child
                    .pointer("/size/weight")
                    .and_then(Value::as_f64)
                    .filter(|n| n.is_finite() && *n > 0.0)
                    .unwrap_or(1.0);
                parts.push((tree, weight));
            }
        }
        if parts.is_empty() {
            return Ok(None);
        }
        if node.get("type").and_then(Value::as_str) == Some("stack") {
            return Ok(Some(SavedNode::Stack {
                panes: parts.iter().flat_map(|(n, _)| n.panes()).collect(),
                active: 0,
            }));
        }
        let axis = if node.get("type").and_then(Value::as_str) == Some("column") {
            SavedAxis::Vertical
        } else {
            SavedAxis::Horizontal
        };
        let (mut tree, mut weight) = parts.pop().unwrap();
        while let Some((first, w)) = parts.pop() {
            tree = SavedNode::Split {
                axis,
                ratio: (w / (w + weight)) as f32,
                first: Box::new(first),
                second: Box::new(tree),
            };
            weight += w;
        }
        return Ok(Some(tree));
    }
    let content = node.get("content").cloned().unwrap_or(Value::Null);
    let terminal = content
        .get("terminal")
        .and_then(Value::as_str)
        .map(|t| qualify(machine, t));
    let id = *next;
    *next += 1;
    panes.insert(
        id.to_string(),
        SavedPane {
            origin: node
                .get("id")
                .and_then(Value::as_str)
                .map(|id| qualify(machine, id)),
            module: terminal.is_none().then_some(node.clone()),
            terminal,
            name: None,
        },
    );
    Ok(Some(SavedNode::Leaf { pane: id }))
}

fn find<'a>(value: &'a Value, id: &str) -> Option<&'a Value> {
    if value.get("id").and_then(Value::as_str) == Some(id) {
        return Some(value);
    }
    match value {
        Value::Array(a) => a.iter().find_map(|v| find(v, id)),
        Value::Object(o) => o.values().find_map(|v| find(v, id)),
        _ => None,
    }
}

pub fn encode(state: &SavedState, machine: &str, base: &Value) -> Value {
    let mut result = if base.is_object() {
        base.clone()
    } else {
        json!({})
    };
    let spaces: Vec<Value> = state
        .workspaces
        .iter()
        .filter(|w| w.machine.as_deref().unwrap_or("") == machine)
        .filter_map(|w| {
            let id = local(machine, w.origin.as_deref()?);
            let mut space = find(base, id).cloned().unwrap_or_else(|| json!({}));
            space["id"] = json!(id);
            space["name"] = json!(w.label);
            space["folder"] = json!(w.folder.as_deref().unwrap_or(""));
            space["tabs"] = Value::Array(
                w.tabs
                    .iter()
                    .filter_map(|t| {
                        let id = local(machine, t.origin.as_deref()?);
                        let mut tab = find(base, id).cloned().unwrap_or_else(|| json!({}));
                        tab["id"] = json!(id);
                        tab["name"] = json!(t.label);
                        let mut root = encode_node(&t.tree, state, machine, base)?;
                        if !t.floating.is_empty() {
                            let mut children = vec![root];
                            children.extend(t.floating.iter().filter_map(|f| {
                                encode_node(&SavedNode::Leaf { pane: f.pane }, state, machine, base)
                            }));
                            root = json!({"type":"row", "children":children});
                        }
                        if root.get("type").and_then(Value::as_str) == Some("block") {
                            root = json!({"type":"row","children":[root]});
                        }
                        if let Some(original) = tab.get("root") {
                            let mut next = 1;
                            let mut original_panes = BTreeMap::new();
                            if let Ok(Some(original_tree)) =
                                decode_node(original, machine, &mut next, &mut original_panes)
                            {
                                if shape(&original_tree, &original_panes)
                                    == shape(&t.tree, &state.panes)
                                    && t.floating.is_empty()
                                {
                                    root = original.clone();
                                } else if let Some(properties) = original.as_object() {
                                    for (key, value) in properties {
                                        if !matches!(
                                            key.as_str(),
                                            "id" | "type" | "children" | "content"
                                        ) {
                                            root[key] = value.clone();
                                        }
                                    }
                                }
                            }
                        }
                        root["id"] = json!(id);
                        tab["root"] = root;
                        Some(tab)
                    })
                    .collect(),
            );
            Some(space)
        })
        .collect();
    result["spaces"] = Value::Array(spaces);
    result
}
fn shape(node: &SavedNode, panes: &BTreeMap<String, SavedPane>) -> Value {
    match node {
        SavedNode::Leaf { pane } => {
            let p = panes.get(&pane.to_string());
            json!({"origin":p.and_then(|p|p.origin.as_deref()),"terminal":p.and_then(|p|p.terminal.as_deref()),"module":p.and_then(|p|p.module.as_ref())})
        }
        SavedNode::Split {
            axis,
            ratio,
            first,
            second,
        } => {
            json!({"axis":axis,"ratio":ratio,"first":shape(first,panes),"second":shape(second,panes)})
        }
        SavedNode::Stack { panes: ids, .. } => {
            json!({"stack":ids.iter().map(|id|shape(&SavedNode::Leaf{pane:*id},panes)).collect::<Vec<_>>()})
        }
    }
}

fn encode_node(node: &SavedNode, state: &SavedState, machine: &str, base: &Value) -> Option<Value> {
    match node {
        SavedNode::Leaf { pane } => {
            let p = state.panes.get(&pane.to_string())?;
            if let Some(module) = &p.module {
                return Some(module.clone());
            }
            let terminal = local(machine, p.terminal.as_deref()?);
            let id = local(machine, p.origin.as_deref()?);
            let mut value = find(base, id).cloned().unwrap_or_else(|| json!({}));
            value["id"] = json!(id);
            value["type"] = json!("block");
            value["content"] = json!({"terminal":terminal});
            Some(value)
        }
        SavedNode::Split {
            axis,
            ratio,
            first,
            second,
        } => {
            let mut a = encode_node(first, state, machine, base)?;
            let mut b = encode_node(second, state, machine, base)?;
            a["size"] = json!({"weight":ratio});
            b["size"] = json!({"weight":1.0-ratio});
            Some(
                json!({"type":if *axis == SavedAxis::Horizontal {"row"} else {"column"},"children":[a,b]}),
            )
        }
        SavedNode::Stack { panes, active } => Some(
            json!({"type":"stack", "active":active,"children":panes.iter().filter_map(|p| encode_node(&SavedNode::Leaf{pane:*p},state,machine,base)).collect::<Vec<_>>()}),
        ),
    }
}

/// Three-way CAS rebase. Never choose one writer silently when both changed a field.
pub fn merge(base: &Value, ours: &Value, theirs: &Value) -> Result<Value, String> {
    if ours == base || ours == theirs {
        return Ok(theirs.clone());
    }
    if theirs == base {
        return Ok(ours.clone());
    }
    if let (Some(b), Some(o), Some(t)) = (base.as_object(), ours.as_object(), theirs.as_object()) {
        let keys: std::collections::BTreeSet<_> =
            b.keys().chain(o.keys()).chain(t.keys()).collect();
        let mut result = serde_json::Map::new();
        for key in keys {
            let value = merge(
                b.get(key).unwrap_or(&Value::Null),
                o.get(key).unwrap_or(&Value::Null),
                t.get(key).unwrap_or(&Value::Null),
            )
            .map_err(|e| format!("{key}.{e}"))?;
            if !value.is_null() {
                result.insert(key.clone(), value);
            }
        }
        return Ok(Value::Object(result));
    }
    if let (Some(b), Some(o), Some(t)) = (base.as_array(), ours.as_array(), theirs.as_array()) {
        if b.iter()
            .chain(o)
            .chain(t)
            .all(|v| v.get("id").and_then(Value::as_str).is_some())
        {
            let ids: std::collections::BTreeSet<_> = b
                .iter()
                .chain(o)
                .chain(t)
                .filter_map(|v| v["id"].as_str())
                .collect();
            let order = if o.iter().map(|v| &v["id"]).collect::<Vec<_>>()
                == b.iter().map(|v| &v["id"]).collect::<Vec<_>>()
            {
                t
            } else {
                o
            };
            let mut result = Vec::new();
            let mut merged = BTreeMap::new();
            for id in ids {
                let get = |a: &Vec<Value>| {
                    a.iter()
                        .find(|v| v["id"] == id)
                        .cloned()
                        .unwrap_or(Value::Null)
                };
                let v = merge(&get(b), &get(o), &get(t))?;
                if !v.is_null() {
                    merged.insert(id.to_string(), v);
                }
            }
            for id in order.iter().chain(t).filter_map(|v| v["id"].as_str()) {
                if let Some(v) = merged.remove(id) {
                    result.push(v);
                }
            }
            result.extend(merged.into_values());
            return Ok(Value::Array(result));
        }
    }
    Err(t("concurrent edits conflict; reload and reapply this change").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn imports_ids_remote_addresses_and_modules() {
        let raw = json!({"spaces":[{"id":"sp1","name":"same","folder":"/repo","tabs":[{"id":"tb1","root":{"type":"row","children":[{"id":"p1","type":"block","content":{"terminal":"t3"}},{"id":"p2","type":"block","content":{"mod":"files","props":{"root":"/repo"}}}]}}]}]});
        let state = decode(&raw, "vps").unwrap();
        assert_eq!(state.workspaces[0].origin.as_deref(), Some("vps/sp1"));
        assert_eq!(state.panes["1"].terminal.as_deref(), Some("vps/t3"));
        let back = encode(&state, "vps", &raw);
        assert_eq!(
            back["spaces"][0]["tabs"][0]["root"]["children"][1]["content"]["mod"],
            "files"
        );
    }
    #[test]
    fn screen_only_state_never_changes_the_shared_document() {
        let raw = json!({"spaces":[{"id":"space","name":"A","folder":"/repo","tabs":[{"id":"tab","name":"one","root":{"type":"row","children":[{"id":"pane","type":"block","content":{"terminal":"t1"}}]}}]}],"custom":{"desktop":"preserve"}});
        let mut state = decode(&raw, "").unwrap();
        let baseline = encode(&state, "", &raw);
        state.active_workspace = 99;
        state.workspaces[0].active_tab = 99;
        state.workspaces[0].tabs[0].focus = 99;
        state.workspaces[0].tabs[0].settled = true;
        assert_eq!(encode(&state, "", &raw), baseline);
        state.workspaces[0].label = "renamed".into();
        let changed = encode(&state, "", &raw);
        assert_eq!(changed["spaces"][0]["name"], "renamed");
        assert_eq!(changed["custom"], raw["custom"]);
    }

    #[test]
    fn conflict_rebases_different_spaces_but_refuses_same_field() {
        let b = json!({"spaces":[{"id":"a","name":"A"},{"id":"b","name":"B"}]});
        let mut o = b.clone();
        o["spaces"][0]["name"] = json!("ours");
        let mut t = b.clone();
        t["spaces"][1]["name"] = json!("theirs");
        let r = merge(&b, &o, &t).unwrap();
        assert_eq!(r["spaces"][0]["name"], "ours");
        assert_eq!(r["spaces"][1]["name"], "theirs");
        t["spaces"][0]["name"] = json!("conflict");
        assert!(merge(&b, &o, &t).is_err());
    }
}
