use async_recursion::async_recursion;
use async_trait::async_trait;
use std::collections::BTreeMap;
use thiserror::Error;

use crate::cache::CachedEntry;
use crate::tree::ContextTree;
use crate::tree::ContextTreeNode;

const MAX_TITLE_LENGTH: usize = 100;
const SUMMARY_THRESHOLD: usize = 100;

#[derive(Clone, Debug)]
pub struct ContextTreeSummarizer<M: PromptModel> {
    model: Option<M>,
}

impl<M: PromptModel> ContextTreeSummarizer<M> {
    pub fn new(model: Option<M>) -> Self {
        Self { model }
    }

    pub async fn summarize_tree(
        &self,
        tree: &mut ContextTree,
        entries: &BTreeMap<i64, CachedEntry>,
    ) {
        for child in tree.root.children.values_mut() {
            self.summarize_node(child, entries).await;
        }

        for entry_node in tree.root.entry_nodes.values_mut() {
            self.summarize_entry_node(entry_node, entries).await;
        }
    }

    #[async_recursion]
    pub async fn summarize_node(
        &self,
        node: &mut ContextTreeNode,
        entries: &BTreeMap<i64, CachedEntry>,
    ) {
        if !node.updated {
            return;
        }

        node.title = node.id.clone();
        let mut ordered_summaries: Vec<(i64, String, String)> = Vec::new();

        for child in node.children.values_mut() {
            self.summarize_node(child, entries).await;
            ordered_summaries.push((
                child.min_entry_id,
                child.title.clone(),
                child.summary.clone(),
            ));
        }

        for entry_node in node.entry_nodes.values_mut() {
            self.summarize_entry_node(entry_node, entries).await;
            ordered_summaries.push((
                entry_node.entry_id(),
                entry_node.title.clone(),
                entry_node.summary.clone(),
            ));
        }

        ordered_summaries.sort_by_key(|(id, _, _)| *id);

        let combined = ordered_summaries
            .iter()
            .map(|(_, title, summary)| format_summary(title, summary))
            .collect::<Vec<String>>()
            .join("\n\n===\n\n");

        if combined.is_empty() {
            node.summary = String::new();
        } else {
            let word_count = combined.split_whitespace().count();
            if word_count > SUMMARY_THRESHOLD {
                node.summary = self.summarize_with_model(&combined).await;
            } else {
                node.summary = combined;
            }
        }

        node.updated = false;
    }

    async fn summarize_entry_node(
        &self,
        entry_node: &mut ContextTreeNode,
        entries: &BTreeMap<i64, CachedEntry>,
    ) {
        if let Some(entry) = entries.get(&entry_node.entry_id()) {
            entry_node.title = clip_text(entry.payload.title.as_str(), MAX_TITLE_LENGTH);
            entry_node.summary = entry.payload.body.clone();
            entry_node.updated = false;
        }
    }

    async fn summarize_with_model(&self, combined: &str) -> String {
        if let Some(model) = &self.model {
            if let Ok(response) = model
                .complete(PromptRequest::new(
                    "Summarize the following context for future turns.",
                    combined.to_string(),
                ))
                .await
            {
                if !response.text.trim().is_empty() {
                    return response.text;
                }
            }
        }

        combined.to_string()
    }
}
#[derive(Debug, Error)]
pub enum PromptModelError {
    #[error("prompt model failed: {message}")]
    Failed { message: String },
}

pub struct PromptRequest {
    pub system_prompt: String,
    pub user_prompt: String,
}

impl PromptRequest {
    pub fn new(system_prompt: impl Into<String>, user_prompt: impl Into<String>) -> Self {
        Self {
            system_prompt: system_prompt.into(),
            user_prompt: user_prompt.into(),
        }
    }
}

pub struct PromptResponse {
    pub text: String,
}

#[async_trait]
pub trait PromptModel: Send + Sync {
    async fn complete(&self, request: PromptRequest) -> Result<PromptResponse, PromptModelError>;
}

fn clip_text(text: &str, limit: usize) -> String {
    let clipped: String = text.chars().take(limit).collect();

    if clipped.len() == text.len() {
        clipped
    } else {
        format!("{}...", clipped.trim_end())
    }
}

fn format_summary(title: &str, summary: &str) -> String {
    let trimmed_summary = summary.trim();
    let trimmed_title = title.trim();

    if trimmed_summary.is_empty() {
        trimmed_title.to_string()
    } else if trimmed_title.is_empty() {
        trimmed_summary.to_string()
    } else {
        format!("{trimmed_title}:\n\n{trimmed_summary}")
    }
}
