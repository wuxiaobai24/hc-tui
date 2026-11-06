use super::*;
use crate::i18n::t;
use std::collections::{HashMap, HashSet};

pub(super) fn arrange(
    mut rows: Vec<NavigationRow>,
    terminals: &[TerminalSummary],
    folded: &HashSet<String>,
    priority: bool,
) -> Vec<NavigationRow> {
    let index: HashMap<_, _> = rows
        .iter()
        .enumerate()
        .filter_map(|(i, r)| r.terminal.clone().map(|a| (a, i)))
        .collect();
    let mut children: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut roots = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        let parent = row
            .terminal
            .as_ref()
            .and_then(|a| terminals.iter().find(|t| &t.address == a))
            .and_then(|t| t.parent.as_ref())
            .and_then(|p| index.get(p))
            .copied();
        if row.group != "Pinned" {
            if let Some(parent) = parent.filter(|p| *p != i) {
                let mut visited = HashSet::from([i]);
                let mut cursor = Some(parent);
                let mut cycle = false;
                while let Some(p) = cursor {
                    if !visited.insert(p) {
                        cycle = true;
                        break;
                    }
                    cursor = rows[p]
                        .terminal
                        .as_ref()
                        .and_then(|a| terminals.iter().find(|t| &t.address == a))
                        .and_then(|t| t.parent.as_ref())
                        .and_then(|p| index.get(p))
                        .copied();
                }
                if !cycle {
                    children.entry(parent).or_default().push(i);
                    continue;
                }
            }
        }
        roots.push(i);
    }
    fn rank(i: usize, rows: &[NavigationRow], children: &HashMap<usize, Vec<usize>>) -> Badge {
        children
            .get(&i)
            .into_iter()
            .flatten()
            .fold(rows[i].badge, |r, c| r.min(rank(*c, rows, children)))
    }
    let ranks: HashMap<_, _> = (0..rows.len())
        .map(|i| (i, rank(i, &rows, &children)))
        .collect();
    if priority {
        for &i in &roots {
            if rows[i].group != "Pinned" {
                rows[i].group = t(attention_section(ranks[&i])).to_string();
                rows[i].group_id = format!("priority:{}", rows[i].group);
            }
        }
    }
    roots.sort_by(|a, b| {
        (rows[*a].group != "Pinned")
            .cmp(&(rows[*b].group != "Pinned"))
            .then_with(|| {
                if priority {
                    ranks[a]
                        .cmp(&ranks[b])
                        .then(rows[*a].group.cmp(&rows[*b].group))
                } else {
                    rows[*a]
                        .group
                        .cmp(&rows[*b].group)
                        .then(rows[*a].group_id.cmp(&rows[*b].group_id))
                        .then(ranks[a].cmp(&ranks[b]))
                }
            })
            .then(a.cmp(b))
    });
    fn append(
        i: usize,
        depth: usize,
        group: (&str, &str),
        rows: &[NavigationRow],
        children: &HashMap<usize, Vec<usize>>,
        folded: &HashSet<String>,
        out: &mut Vec<NavigationRow>,
    ) {
        let mut row = rows[i].clone();
        row.group = group.0.to_string();
        row.group_id = group.1.to_string();
        let count = children.get(&i).map(Vec::len).unwrap_or(0);
        let key = format!("agent:{}", row.terminal.as_deref().unwrap_or(""));
        let prefix = if depth > 0 {
            format!("{}↳ ", "  ".repeat(depth.saturating_sub(1)))
        } else {
            String::new()
        };
        row.label = if count > 0 {
            format!(
                "{prefix}{} {} ({count})",
                if folded.contains(&key) { "▸" } else { "▾" },
                row.label
            )
        } else {
            format!("{prefix}{}", row.label)
        };
        out.push(row);
        if !folded.contains(&key) {
            if let Some(kids) = children.get(&i) {
                let mut kids = kids.clone();
                kids.sort_by_key(|c| (rows[*c].badge, *c));
                for child in kids {
                    append(child, depth + 1, group, rows, children, folded, out);
                }
            }
        }
    }
    let mut result = Vec::new();
    for root in roots {
        append(
            root,
            0,
            (&rows[root].group, &rows[root].group_id),
            &rows,
            &children,
            folded,
            &mut result,
        );
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn child_attention_moves_parent_and_folds_without_duplicates() {
        let ts: Vec<_> = [
            json!({"address":"t1"}),
            json!({"address":"t2","parent":"t1"}),
            json!({"address":"t3"}),
        ]
        .iter()
        .map(|v| TerminalSummary::parse(v).unwrap())
        .collect();
        let row = |a: &str, b| NavigationRow {
            group: "Idle".into(),
            group_id: "idle".into(),
            pane: None,
            terminal: Some(a.to_string()),
            badge: b,
            label: a.to_string(),
        };
        let rows = vec![
            row("t3", Badge::Working),
            row("t1", Badge::Idle),
            row("t2", Badge::Blocked),
        ];
        let arranged = arrange(rows.clone(), &ts, &HashSet::new(), true);
        assert_eq!(arranged[0].terminal.as_deref(), Some("t1"));
        assert_eq!(arranged[1].terminal.as_deref(), Some("t2"));
        assert_eq!(arranged[0].group, "Needs you");
        let folded = arrange(rows, &ts, &HashSet::from(["agent:t1".to_string()]), true);
        assert_eq!(folded.len(), 2);
    }
    #[test]
    fn corrupt_parent_cycle_is_bounded() {
        let ts: Vec<_> = [
            json!({"address":"t1","parent":"t2"}),
            json!({"address":"t2","parent":"t1"}),
        ]
        .iter()
        .map(|v| TerminalSummary::parse(v).unwrap())
        .collect();
        let rows = ts
            .iter()
            .map(|t| NavigationRow {
                group: "Idle".into(),
                group_id: "idle".into(),
                pane: None,
                terminal: Some(t.address.clone()),
                badge: Badge::Idle,
                label: t.address.clone(),
            })
            .collect();
        assert_eq!(arrange(rows, &ts, &HashSet::new(), true).len(), 2);
    }
}
