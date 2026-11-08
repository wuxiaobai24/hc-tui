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

/// 数组元素的合并键：`id` 是字符串或数字都认（`SavedState` 的 id 是数字）。
fn list_key(value: &Value) -> Option<String> {
    let id = value.get("id")?;
    Some(match id.as_str() {
        Some(text) => text.to_string(),
        None => id.to_string(),
    })
}

fn number(value: Option<&Value>) -> u32 {
    value.and_then(Value::as_u64).unwrap_or(0) as u32
}

fn origin(value: &Value) -> Option<&str> {
    value.get("origin").and_then(Value::as_str)
}

/// 同一块屏幕的两个表示：`origin` 优先，退化为终端地址。
fn same_pane(base: &Value, theirs: &Value) -> bool {
    match (origin(base), origin(theirs)) {
        (Some(a), Some(b)) => a == b,
        _ => match (
            base.get("terminal").and_then(Value::as_str),
            theirs.get("terminal").and_then(Value::as_str),
        ) {
            (Some(a), Some(b)) => a == b,
            _ => false,
        },
    }
}

/// 把文档里的一个数组元素配到 `base` 的本地编号上：有 `origin` 就按它认身份，
/// 双方都没有 `origin` 时按第一个还没被认领的无名元素（同一份文档的同一位置）。
fn match_entry<'a>(
    base: impl Iterator<Item = (u32, &'a Value)>,
    theirs: &Value,
    claimed: &std::collections::HashSet<u32>,
) -> Option<u32> {
    let mut fallback = None;
    for (id, entry) in base {
        if claimed.contains(&id) {
            continue;
        }
        if let (Some(a), Some(b)) = (origin(entry), origin(theirs)) {
            if a == b {
                return Some(id);
            }
            continue;
        }
        if origin(entry).is_none() && origin(theirs).is_none() && fallback.is_none() {
            fallback = Some(id);
        }
    }
    fallback
}

fn map_refs(value: &mut Value, panes: &BTreeMap<u32, u32>) -> Result<(), String> {
    let miss = || t("layout references a pane missing from the document").to_string();
    let map = |id: u32| panes.get(&id).copied().ok_or_else(miss);
    match value {
        Value::Object(map_obj) => {
            if let Some(pane) = map_obj.get("pane").and_then(Value::as_u64) {
                let mapped = map(pane as u32)?;
                map_obj.insert("pane".to_string(), json!(mapped));
            }
            if let Some(focus) = map_obj.get("focus").and_then(Value::as_u64) {
                let mapped = map(focus as u32)?;
                map_obj.insert("focus".to_string(), json!(mapped));
            }
            for key in ["first", "second", "tree"] {
                if let Some(child) = map_obj.get_mut(key) {
                    map_refs(child, panes)?;
                }
            }
            for key in ["panes", "floating"] {
                for item in map_obj
                    .get_mut(key)
                    .and_then(Value::as_array_mut)
                    .into_iter()
                    .flatten()
                {
                    if let Some(id) = item.as_u64() {
                        *item = json!(map(id as u32)?);
                    } else {
                        map_refs(item, panes)?;
                    }
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// 把文档编号（`theirs`）翻译进 `base` 的本地编号空间。
///
/// `hctui-layout-v1` 里的 pane / tab / workspace 编号是写入方进程内的编号，
/// 换一个写入者就对不上；`origin` 是跨进程的身份。三方合并先按 `origin`
/// （没有时按无名元素的位置）把远端值重编号，base / ours / theirs 才可比。
pub fn translate(theirs: &Value, base: &Value) -> Result<Value, String> {
    translate_beyond(theirs, base, &Value::Null)
}

/// 同 [`translate`]，但新实体的本地编号从 `used` 的最大编号之后开始。
///
/// 三方合并的另一侧（`ours`）可能已经占用了 base 之后的编号；两边各自新建的
/// pane/tab/space 若拿到同一个 id，通用合并会把它们当成同一项的内容冲突。
pub fn translate_beyond(theirs: &Value, base: &Value, used: &Value) -> Result<Value, String> {
    let base_workspaces = base
        .get("workspaces")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let base_panes = base
        .get("panes")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut next = 1;
    for source in [base, used] {
        for workspace in source
            .get("workspaces")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            next = next.max(number(workspace.get("id")) + 1);
            for tab in workspace
                .get("tabs")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                next = next.max(number(tab.get("id")) + 1);
            }
        }
        for id in source
            .get("panes")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
            .map(|(id, _)| id)
        {
            next = next.max(id.parse::<u32>().unwrap_or(0) + 1);
        }
    }

    let mut panes = serde_json::Map::new();
    let mut pane_ids = BTreeMap::new();
    let mut claimed_pane_keys: std::collections::HashSet<String> = Default::default();
    let base_pane_ids: Vec<u32> = base_panes
        .keys()
        .filter_map(|id| id.parse::<u32>().ok())
        .collect();
    for (doc_id, pane) in theirs
        .get("panes")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        let matched = base_panes
            .iter()
            .filter(|(id, _)| !claimed_pane_keys.contains(*id))
            .find(|(_, entry)| same_pane(entry, pane))
            .map(|(id, _)| id.clone());
        let matched = matched.or_else(|| {
            // 双方都没有身份（手写或旧版文档）：按还没认领的第一个位置配对。
            if origin(pane).is_some() {
                return None;
            }
            base_pane_ids
                .iter()
                .find(|id| {
                    let key = id.to_string();
                    !claimed_pane_keys.contains(&key)
                        && base_panes
                            .get(&key)
                            .is_some_and(|entry| origin(entry).is_none())
                })
                .map(u32::to_string)
        });
        let local = matched.unwrap_or_else(|| {
            let id = next;
            next += 1;
            id.to_string()
        });
        claimed_pane_keys.insert(local.clone());
        if let (Ok(doc_id), Ok(local_id)) = (doc_id.parse::<u32>(), local.parse::<u32>()) {
            pane_ids.insert(doc_id, local_id);
        }
        panes.insert(local, pane.clone());
    }

    let mut claimed_workspaces = std::collections::HashSet::new();
    let mut claimed_tabs = std::collections::HashSet::new();
    let base_entries: Vec<(u32, &Value)> = base_workspaces
        .iter()
        .map(|workspace| (number(workspace.get("id")), workspace))
        .collect();
    let mut workspaces = Vec::new();
    for workspace in theirs
        .get("workspaces")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let local = match_entry(base_entries.iter().copied(), workspace, &claimed_workspaces)
            .unwrap_or_else(|| {
                let id = next;
                next += 1;
                id
            });
        claimed_workspaces.insert(local);
        let mut workspace = workspace.clone();
        workspace["id"] = json!(local);
        let base_tabs: Vec<(u32, &Value)> = base_entries
            .iter()
            .find(|(id, _)| *id == local)
            .and_then(|(_, entry)| entry.get("tabs").and_then(Value::as_array))
            .into_iter()
            .flatten()
            .map(|tab| (number(tab.get("id")), tab))
            .collect();
        let mut tabs = Vec::new();
        for tab in workspace
            .get("tabs")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let tab_id =
                match_entry(base_tabs.iter().copied(), tab, &claimed_tabs).unwrap_or_else(|| {
                    let id = next;
                    next += 1;
                    id
                });
            claimed_tabs.insert(tab_id);
            let mut tab = tab.clone();
            tab["id"] = json!(tab_id);
            map_refs(&mut tab, &pane_ids)?;
            tabs.push(tab);
        }
        workspace["tabs"] = Value::Array(tabs);
        workspaces.push(workspace);
    }

    let mut result = theirs.clone();
    result["workspaces"] = Value::Array(workspaces);
    result["panes"] = Value::Object(panes);
    Ok(result)
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
        if b.iter().chain(o).chain(t).all(|v| list_key(v).is_some()) {
            let ids: std::collections::BTreeSet<_> =
                b.iter().chain(o).chain(t).filter_map(list_key).collect();
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
                        .find(|v| list_key(v).as_deref() == Some(id.as_str()))
                        .cloned()
                        .unwrap_or(Value::Null)
                };
                let v = merge(&get(b), &get(o), &get(t))?;
                if !v.is_null() {
                    merged.insert(id, v);
                }
            }
            for id in order.iter().chain(t).filter_map(list_key) {
                if let Some(v) = merged.remove(&id) {
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
    fn module_nodes_keep_unknown_fields_props_and_look() {
        let raw = json!({
            "spaces":[{
                "id":"sp1","name":"A","folder":"/repo","look":{"density":"cozy"},"extra_space":{"n":7},
                "tabs":[{
                    "id":"tb1","look":{"tab":"top"},
                    "root":{"type":"row","look":{"row":1},"children":[
                        {"id":"p1","type":"block","content":{"terminal":"t3"},"look":{"border":"rounded"},"size":{"weight":0.6},"unknown_node":{"deep":[1,2]}},
                        {"id":"p2","type":"block","content":{"mod":"files","props":{"root":"/repo","unknown":{"deep":[true,null,3]}},"custom":"keep"},"look":{"surface":"docked"},"meta":{"n":1},"size":{"weight":0.4}}
                    ]}
                }]
            }],
            "extra":{"top":true}
        });
        let state = decode(&raw, "").unwrap();
        let back = encode(&state, "", &raw);
        let nodes = &back["spaces"][0]["tabs"][0]["root"]["children"];
        assert_eq!(nodes[0], raw["spaces"][0]["tabs"][0]["root"]["children"][0]);
        assert_eq!(nodes[1], raw["spaces"][0]["tabs"][0]["root"]["children"][1]);
        assert_eq!(
            nodes[1]["content"]["props"]["unknown"]["deep"][1],
            Value::Null
        );
        assert_eq!(back["spaces"][0]["look"], raw["spaces"][0]["look"]);
        assert_eq!(back["extra"], raw["extra"]);
        assert_eq!(back["spaces"][0]["extra_space"], json!({"n":7}));
    }

    #[test]
    fn props_merge_per_module_and_conflicts_name_the_path() {
        let base = json!({"spaces":[{"id":"a","tabs":[{"id":"t","root":{"type":"row","children":[
            {"id":"p1","type":"block","content":{"mod":"files","props":{"root":"/a"}}},
            {"id":"p2","type":"block","content":{"mod":"prs","props":{"repo":"x"}}}
        ]}}]}]});
        let mut ours = base.clone();
        ours["spaces"][0]["tabs"][0]["root"]["children"][0]["content"]["props"]["root"] =
            json!("/ours");
        let mut theirs = base.clone();
        theirs["spaces"][0]["tabs"][0]["root"]["children"][1]["content"]["props"]["repo"] =
            json!("theirs");
        let merged = merge(&base, &ours, &theirs).unwrap();
        assert_eq!(
            merged["spaces"][0]["tabs"][0]["root"]["children"][0]["content"]["props"]["root"],
            json!("/ours")
        );
        assert_eq!(
            merged["spaces"][0]["tabs"][0]["root"]["children"][1]["content"]["props"]["repo"],
            json!("theirs")
        );
        let mut clash = base.clone();
        clash["spaces"][0]["tabs"][0]["root"]["children"][0]["content"]["props"]["root"] =
            json!("/clash");
        let error = merge(&base, &ours, &clash).unwrap_err();
        assert!(error.contains("props"), "{error}");
        assert!(error.contains("root"), "{error}");
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
    #[test]
    fn numeric_ids_merge_as_a_union_like_string_ids() {
        // `hctui-layout-v1` 的 workspace / tab id 是数字：数组仍按 id 合并。
        let b = json!({"workspaces":[{"id":1,"label":"","tabs":[{"id":1,"label":"one"}]}]});
        let mut o = b.clone();
        o["workspaces"][0]["tabs"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":2,"label":"two"}));
        let t = json!({"workspaces":[{"id":1,"label":"EXTERNAL","tabs":[{"id":1,"label":"one"}]}]});
        let r = merge(&b, &o, &t).unwrap();
        assert_eq!(r["workspaces"][0]["label"], "EXTERNAL");
        assert_eq!(r["workspaces"][0]["tabs"].as_array().unwrap().len(), 2);
        assert_eq!(r["workspaces"][0]["tabs"][0]["label"], "one");
        assert_eq!(r["workspaces"][0]["tabs"][1]["label"], "two");
        // 数字 id 上的同一字段被两边改动时仍然报冲突并指出路径。
        let mut clash = t.clone();
        clash["workspaces"][0]["tabs"][0]["label"] = json!("theirs");
        let mut ours = b.clone();
        ours["workspaces"][0]["tabs"][0]["label"] = json!("ours");
        let error = merge(&b, &ours, &clash).unwrap_err();
        assert!(error.contains("label"), "{error}");
    }
    #[test]
    fn string_ids_keep_the_same_union_and_conflict_behaviour() {
        let b = json!({"spaces":[{"id":"a","name":"A","tabs":[{"id":"t1","name":"one"}]}]});
        let mut o = b.clone();
        o["spaces"][0]["tabs"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":"t2","name":"two"}));
        let t = json!({"spaces":[{"id":"a","name":"EXTERNAL","tabs":[{"id":"t1","name":"one"}]}]});
        let r = merge(&b, &o, &t).unwrap();
        assert_eq!(r["spaces"][0]["name"], "EXTERNAL");
        assert_eq!(r["spaces"][0]["tabs"].as_array().unwrap().len(), 2);
        let mut clash = t.clone();
        clash["spaces"][0]["tabs"][0]["name"] = json!("theirs");
        let mut ours = b.clone();
        ours["spaces"][0]["tabs"][0]["name"] = json!("ours");
        let error = merge(&b, &ours, &clash).unwrap_err();
        assert!(error.contains("name"), "{error}");
    }
    #[test]
    fn a_foreign_document_is_translated_into_the_base_numbering() {
        // `base` 是本地编号（pane 2/3、tab 1），文档里是写入方的编号（pane 1/2）。
        let base = json!({"schema_version":3,"workspaces":[{
            "id":7,"label":"mine","active_tab":0,"tabs":[{
                "id":5,"label":"","focus":3,"settled":false,"floating":[],
                "tree":{"kind":"split","axis":"horizontal","ratio":0.5,
                        "first":{"kind":"leaf","pane":2},"second":{"kind":"leaf","pane":3}}
            }]}],
            "panes":{"2":{"origin":null,"terminal":null,"module":null,"name":null},
                     "3":{"origin":null,"terminal":null,"module":null,"name":null}}});
        let theirs = json!({"schema_version":3,"workspaces":[{
            "id":1,"label":"EXTERNAL","active_tab":0,"tabs":[{
                "id":1,"label":"ext","focus":1,"settled":false,"floating":[],
                "tree":{"kind":"leaf","pane":1}
            }]}],
            "panes":{"1":{"origin":null,"terminal":null,"module":null,"name":null}}});
        let translated = translate(&theirs, &base).unwrap();
        assert_eq!(translated["workspaces"][0]["id"], 7);
        assert_eq!(translated["workspaces"][0]["tabs"][0]["id"], 5);
        assert_eq!(translated["workspaces"][0]["tabs"][0]["focus"], 2);
        assert_eq!(translated["workspaces"][0]["tabs"][0]["tree"]["pane"], 2);
        // 合并后：外部的 label 与本地多出来的 tab 都在。
        let mut ours = base.clone();
        ours["workspaces"][0]["tabs"].as_array_mut().unwrap().push(
            json!({"id":9,"label":"","focus":3,"settled":false,"floating":[],
                         "tree":{"kind":"leaf","pane":3}}),
        );
        let merged = merge(&base, &ours, &translated).unwrap();
        assert_eq!(merged["workspaces"][0]["label"], "EXTERNAL");
        assert_eq!(merged["workspaces"][0]["tabs"].as_array().unwrap().len(), 2);
        assert!(merged["workspaces"][0]["tabs"][0]["tree"]
            .get("pane")
            .is_some());
    }
    #[test]
    fn translate_keeps_identity_by_origin_across_renumbered_documents() {
        let base = json!({"workspaces":[{"id":4,"label":"a","origin":"w1","tabs":[
            {"id":9,"label":"t","origin":"t1","tree":{"kind":"leaf","pane":11}}]}],
            "panes":{"11":{"origin":"p1","terminal":"tty1"}}});
        let theirs = json!({"workspaces":[{"id":1,"label":"b","origin":"w1","tabs":[
            {"id":2,"label":"t","origin":"t1","tree":{"kind":"leaf","pane":3}}]}],
            "panes":{"3":{"origin":"p1","terminal":"tty1"}}});
        let translated = translate(&theirs, &base).unwrap();
        assert_eq!(translated["workspaces"][0]["id"], 4);
        assert_eq!(translated["workspaces"][0]["tabs"][0]["id"], 9);
        assert_eq!(
            translated["panes"]
                .as_object()
                .unwrap()
                .keys()
                .next()
                .unwrap(),
            "11"
        );
        // 同一份文档去掉身份字段后仍然按位置译回同一套编号。
        let mut anonymous = theirs.clone();
        anonymous["workspaces"][0]["origin"] = Value::Null;
        anonymous["workspaces"][0]["tabs"][0]["origin"] = Value::Null;
        anonymous["panes"]["3"]["origin"] = Value::Null;
        let mut plain = base.clone();
        plain["workspaces"][0]["origin"] = Value::Null;
        plain["workspaces"][0]["tabs"][0]["origin"] = Value::Null;
        plain["panes"]["11"]["origin"] = Value::Null;
        let translated = translate(&anonymous, &plain).unwrap();
        assert_eq!(translated["workspaces"][0]["id"], 4);
        assert_eq!(
            translated["panes"]
                .as_object()
                .unwrap()
                .keys()
                .next()
                .unwrap(),
            "11"
        );
    }

    #[test]
    fn new_entities_of_both_writers_get_disjoint_ids() {
        let base = json!({
            "workspaces": [{"id": 1, "label": "", "active_tab": 0, "tabs": [
                {"id": 1, "label": "a", "tree": {"kind": "leaf", "pane": 1}}]}],
            "panes": {"1": {"origin": "p1", "terminal": "t1"}}
        });
        let ours = json!({
            "workspaces": [{"id": 1, "label": "", "active_tab": 1, "tabs": [
                {"id": 1, "label": "a", "tree": {"kind": "leaf", "pane": 1}},
                {"id": 3, "label": "ours", "tree": {"kind": "leaf", "pane": 3}}]}],
            "panes": {
                "1": {"origin": "p1", "terminal": "t1"},
                "3": {"origin": "p3", "terminal": "t3"}
            }
        });
        let theirs = json!({
            "workspaces": [{"id": 1, "label": "", "active_tab": 1, "tabs": [
                {"id": 1, "label": "a", "tree": {"kind": "leaf", "pane": 1}},
                {"id": 2, "label": "theirs", "tree": {"kind": "leaf", "pane": 2}}]}],
            "panes": {
                "1": {"origin": "p1", "terminal": "t1"},
                "2": {"origin": "p2", "terminal": "t2"}
            }
        });
        // 带上 ours 的编号：远端新 pane 从 ours 之后开始，不会与 ours 的新 pane 撞号。
        let translated = translate_beyond(&theirs, &base, &ours).unwrap();
        let mut ids: Vec<u32> = translated["panes"]
            .as_object()
            .unwrap()
            .keys()
            .map(|id| id.parse().unwrap())
            .collect();
        ids.sort_unstable();
        assert_eq!(ids, vec![1, 4]);
        let merged = merge(&base, &ours, &translated).unwrap();
        assert_eq!(merged["panes"].as_object().unwrap().len(), 3);
        assert_eq!(merged["workspaces"][0]["tabs"].as_array().unwrap().len(), 3);
    }
}
