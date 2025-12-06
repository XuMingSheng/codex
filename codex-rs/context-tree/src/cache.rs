use std::collections::BTreeMap;
use std::collections::HashMap;

use crate::recorder::EntryPayload;
use crate::recorder::HistoryMutation;
use crate::summarizer::ContextTreeSummarizer;
use crate::summarizer::PromptModel;
use crate::tree::ContextTree;
use crate::tree::ContextTreeNode;

#[derive(Clone, Debug)]
pub struct CachedEntry {
    pub id: i64,
    pub labels: Vec<String>,
    pub payload: EntryPayload,
}

#[derive(Clone, Debug)]
pub struct ContextTreeCache {
    entries: BTreeMap<i64, CachedEntry>,
    tree: ContextTree,
}

impl Default for ContextTreeCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ContextTreeCache {
    pub fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
            tree: ContextTree {
                root: ContextTreeNode::new("root", vec![]),
            },
        }
    }

    pub fn selected(&self) -> Vec<i64> {
        self.tree.get_selected_entry_ids()
    }

    pub fn tree(&self) -> &ContextTree {
        &self.tree
    }

    pub fn toggle_selected(&mut self, node_ids: &[String]) {
        self.tree.toggle_selected(node_ids);
    }

    pub fn toggle_collapsed(&mut self, node_ids: &[String]) {
        self.tree.toggle_collapsed(node_ids);
    }

    pub fn entry_ids(&self) -> Vec<i64> {
        self.entries.keys().copied().collect()
    }

    pub async fn apply_mutations<M: PromptModel>(
        &mut self,
        summarizer: &ContextTreeSummarizer<M>,
        mutations: Vec<HistoryMutation>,
    ) {
        if mutations.is_empty() {
            return;
        }

        let mut index_changes = HashMap::new();
        let mut removes = Vec::new();
        let mut inserts = Vec::new();

        for mutation in mutations {
            match mutation {
                HistoryMutation::IndexChange {
                    old_index,
                    new_index,
                } => {
                    index_changes.insert(old_index as i64, new_index as i64);
                }
                HistoryMutation::Remove { index } => {
                    removes.push(index as i64);
                }
                HistoryMutation::Insert {
                    index,
                    labels,
                    payload,
                } => {
                    inserts.push(CachedEntry {
                        id: index as i64,
                        labels,
                        payload,
                    });
                }
            }
        }

        if !removes.is_empty() {
            self.apply_removes(&removes);
        }

        if !index_changes.is_empty() {
            self.apply_index_changes(&index_changes);
        }

        if !inserts.is_empty() {
            self.apply_inserts(&inserts);
        }

        summarizer
            .summarize_tree(&mut self.tree, &self.entries)
            .await;
    }

    fn apply_removes(&mut self, removes: &[i64]) {
        for id in removes {
            self.entries.remove(id);
            self.tree.remove_entry_node(*id);
        }
    }

    fn apply_index_changes(&mut self, changes: &HashMap<i64, i64>) {
        self.entries = self
            .entries
            .iter()
            .map(|(id, entry)| {
                if let Some(new_id) = changes.get(id) {
                    let new_entry = entry.clone();
                    (*new_id, new_entry)
                } else {
                    (*id, entry.clone())
                }
            })
            .collect();

        self.tree.rename_entry_nodes(changes);
    }

    fn apply_inserts(&mut self, inserts: &[CachedEntry]) {
        for entry in inserts {
            self.entries.insert(entry.id, entry.clone());
            self.tree.insert_entry_node(entry);
        }
    }
}
