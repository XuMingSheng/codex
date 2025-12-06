use std::collections::HashSet;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EntryRole {
    User,
    Assistant,
    Tool,
    Other,
}

/// Minimal payload describing a recorded entry. Core extracts this from `ResponseItem`s before
/// passing it into the mutation recorder so the crate stays protocol agnostic.
#[derive(Clone, Debug)]
pub struct EntryPayload {
    pub role: EntryRole,
    pub title: String,
    pub body: String,
}

impl EntryPayload {
    pub fn new(role: EntryRole, title: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            role,
            title: title.into(),
            body: body.into(),
        }
    }
}

#[derive(Clone, Debug)]
enum MutatedEntry {
    Mirror {
        original_index: usize,
    },
    Inserted {
        labels: Vec<String>,
        payload: EntryPayload,
    },
}

/// Describes a mutation applied to the ordered history vector.
#[derive(Clone, Debug)]
pub enum HistoryMutation {
    IndexChange {
        old_index: usize,
        new_index: usize,
    },
    Insert {
        index: usize,
        labels: Vec<String>,
        payload: EntryPayload,
    },
    Remove {
        index: usize,
    },
}

/// Collects incoming mutations until callers decide to commit them to the cache.
#[derive(Clone, Debug)]
pub struct HistoryMutationRecorder {
    current_history_len: usize,
    mutated_history: Vec<MutatedEntry>,
}

impl HistoryMutationRecorder {
    pub fn new(history_len: usize) -> Self {
        let mut mutated_history = Vec::with_capacity(history_len);
        for index in 0..history_len {
            mutated_history.push(MutatedEntry::Mirror {
                original_index: index,
            });
        }

        Self {
            current_history_len: history_len,
            mutated_history,
        }
    }

    pub fn insert(&mut self, index: usize, labels: Vec<String>, payload: EntryPayload) {
        self.mutated_history
            .insert(index, MutatedEntry::Inserted { labels, payload });
    }

    pub fn remove(&mut self, index: usize) {
        self.mutated_history.remove(index);
    }

    pub fn replace(&mut self, index: usize, labels: Vec<String>, payload: EntryPayload) {
        self.remove(index);
        self.insert(index, labels, payload);
    }

    pub fn clear(&mut self) {
        self.mutated_history.clear();
    }

    pub fn commit(&mut self) -> Vec<HistoryMutation> {
        let mut mutations = vec![];
        let mut original_indices = HashSet::new();

        for (index, entry) in self.mutated_history.iter().enumerate() {
            match entry {
                MutatedEntry::Mirror { original_index } => {
                    original_indices.insert(*original_index);

                    if *original_index != index {
                        mutations.push(HistoryMutation::IndexChange {
                            old_index: *original_index,
                            new_index: index,
                        });
                    }
                }
                MutatedEntry::Inserted { labels, payload } => {
                    mutations.push(HistoryMutation::Insert {
                        index,
                        labels: labels.clone(),
                        payload: payload.clone(),
                    });
                }
            }
        }

        for original_index in 0..self.current_history_len {
            if !original_indices.contains(&original_index) {
                mutations.push(HistoryMutation::Remove {
                    index: original_index,
                });
            }
        }

        self.reset_mutated_history(self.mutated_history.len());
        mutations
    }

    fn reset_mutated_history(&mut self, history_len: usize) {
        self.current_history_len = history_len;
        self.mutated_history.clear();
        for index in 0..history_len {
            self.mutated_history.push(MutatedEntry::Mirror {
                original_index: index,
            });
        }
    }
}

impl Default for HistoryMutationRecorder {
    fn default() -> Self {
        Self::new(0)
    }
}
