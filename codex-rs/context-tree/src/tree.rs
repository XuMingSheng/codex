use std::collections::BTreeMap;
use std::collections::HashMap;

use crate::cache::CachedEntry;

#[derive(Clone, Debug)]
pub struct ContextTreeNode {
    pub id: String,
    pub labels: Vec<String>,
    pub updated: bool,
    pub children: BTreeMap<String, ContextTreeNode>,
    pub entry_nodes: BTreeMap<String, ContextTreeNode>,

    pub title: String,
    pub summary: String,
    pub selected: bool,
    pub collapsed: bool,
    pub min_entry_id: i64,
}

#[derive(Clone, Debug)]
pub struct ContextTree {
    pub root: ContextTreeNode,
}

impl ContextTreeNode {
    pub fn new(id: &str, labels: Vec<String>) -> Self {
        Self {
            id: id.to_string(),
            labels,
            updated: true,
            children: BTreeMap::new(),
            entry_nodes: BTreeMap::new(),
            title: "".to_string(),
            summary: "".to_string(),
            selected: true,
            collapsed: true,
            min_entry_id: if id.starts_with("entry-") {
                id.strip_prefix("entry-")
                    .and_then(|s| s.parse::<i64>().ok())
                    .unwrap_or(i64::MAX)
            } else {
                i64::MAX
            },
        }
    }

    pub fn is_entry(&self) -> bool {
        self.id.starts_with("entry-")
    }

    pub fn entry_id(&self) -> i64 {
        self.id
            .strip_prefix("entry-")
            .unwrap_or(&self.id)
            .parse::<i64>()
            .unwrap_or(0)
    }

    pub fn recompute_min_entry_id(&mut self) {
        if self.is_entry() {
            self.min_entry_id = self.entry_id();
            return;
        }

        let mut min_id = i64::MAX;

        for entry_node in self.entry_nodes.values() {
            min_id = min_id.min(entry_node.entry_id());
        }

        for child in self.children.values() {
            min_id = min_id.min(child.min_entry_id);
        }

        self.min_entry_id = min_id;
    }
}

impl ContextTree {
    pub fn insert_entry_node(&mut self, entry: &CachedEntry) {
        let node = ContextTreeNode::new(into_node_id(entry.id).as_str(), entry.labels.clone());

        insert_entry_into_tree(&mut self.root, node, 0);
    }

    pub fn remove_entry_node(&mut self, entry_id: i64) {
        let node_id = into_node_id(entry_id);
        remove_entry_from_tree(&mut self.root, node_id.as_str());
    }

    pub fn rename_entry_nodes(&mut self, renames: &HashMap<i64, i64>) {
        let node_id_renames: HashMap<String, String> = renames
            .iter()
            .map(|(old_id, new_id)| (into_node_id(*old_id), into_node_id(*new_id)))
            .collect();

        rename_entries_in_tree(&mut self.root, &node_id_renames);
    }

    pub fn get_selected_entry_ids(&self) -> Vec<i64> {
        let mut selected_ids = Vec::new();
        collect_selected_entry_ids(&self.root, &mut selected_ids);
        selected_ids
    }

    pub fn toggle_selected(&mut self, node_ids: &[String]) {
        toggle_node_selected(&mut self.root, node_ids);
    }

    pub fn toggle_collapsed(&mut self, node_ids: &[String]) {
        toggle_node_collapsed(&mut self.root, node_ids);
    }
}

fn insert_entry_into_tree(node: &mut ContextTreeNode, entry_node: ContextTreeNode, depth: usize) {
    if depth >= entry_node.labels.len() {
        let entry_id = entry_node.entry_id();
        node.entry_nodes.insert(entry_node.id.clone(), entry_node);
        node.updated = true;
        node.min_entry_id = node.min_entry_id.min(entry_id);
        return;
    }

    let entry_label = entry_node.labels[depth].as_str();

    if let Some(child) = node.children.get_mut(entry_label) {
        insert_entry_into_tree(child, entry_node, depth + 1);
        node.min_entry_id = node.min_entry_id.min(child.min_entry_id);
    } else {
        let mut labels = node.labels.clone();
        labels.push(entry_label.to_string());
        let node_id = format!("/{}", labels.join("/"));

        node.children.insert(
            entry_label.to_string(),
            ContextTreeNode::new(node_id.as_str(), labels),
        );

        if let Some(new_child) = node.children.get_mut(entry_label) {
            insert_entry_into_tree(new_child, entry_node, depth + 1);
            node.min_entry_id = node.min_entry_id.min(new_child.min_entry_id);
        }
    }

    node.updated = true;
}

pub fn into_node_id(entry_id: i64) -> String {
    format!("entry-{entry_id}")
}

fn remove_entry_from_tree(node: &mut ContextTreeNode, target: &str) -> bool {
    if node.entry_nodes.remove(target).is_some() {
        node.updated = true;
        node.recompute_min_entry_id();
        return true;
    }

    let child_labels: Vec<String> = node.children.keys().cloned().collect();
    for label in child_labels {
        if let Some(child) = node.children.get_mut(&label) {
            if remove_entry_from_tree(child, target) {
                node.children.remove(&label);
                node.updated = true;
                break;
            } else {
                node.updated |= child.updated;
            }
        }
    }
    node.recompute_min_entry_id();
    node.children.is_empty() && node.entry_nodes.is_empty()
}

fn rename_entries_in_tree(node: &mut ContextTreeNode, renames: &HashMap<String, String>) {
    node.entry_nodes = node
        .entry_nodes
        .iter()
        .map(|(old_id, entry)| {
            if let Some(new_id) = renames.get(old_id) {
                let mut new_entry = entry.clone();
                new_entry.id = new_id.clone();
                (new_id.clone(), new_entry)
            } else {
                (old_id.clone(), entry.clone())
            }
        })
        .collect(); // Collect into the new map

    for child in node.children.values_mut() {
        rename_entries_in_tree(child, renames);
    }

    node.recompute_min_entry_id();
}

fn collect_selected_entry_ids(node: &ContextTreeNode, selected_ids: &mut Vec<i64>) {
    for entry_node in node.entry_nodes.values() {
        if entry_node.selected {
            selected_ids.push(entry_node.entry_id());
        }
    }

    for child in node.children.values() {
        collect_selected_entry_ids(child, selected_ids);
    }
}

fn toggle_node_selected(node: &mut ContextTreeNode, node_ids: &[String]) {
    if node_ids.contains(&node.id) {
        node.selected = !node.selected;
    }

    for entry_node in node.entry_nodes.values_mut() {
        if node_ids.contains(&entry_node.id) {
            entry_node.selected = !entry_node.selected;
        }
    }

    for child in node.children.values_mut() {
        toggle_node_selected(child, node_ids);
    }
}

fn toggle_node_collapsed(node: &mut ContextTreeNode, node_ids: &[String]) {
    if node_ids.contains(&node.id) {
        node.collapsed = !node.collapsed;
    }

    for entry_node in node.entry_nodes.values_mut() {
        if node_ids.contains(&entry_node.id) {
            entry_node.collapsed = !entry_node.collapsed;
        }
    }

    for child in node.children.values_mut() {
        toggle_node_collapsed(child, node_ids);
    }
}
