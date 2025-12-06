use crate::client::ModelClient;
use crate::client_common::Prompt;
use crate::client_common::ResponseEvent;
use crate::codex::TurnContext;
use crate::context_manager::normalize;
use crate::truncate::TruncationPolicy;
use crate::truncate::approx_token_count;
use crate::truncate::approx_tokens_from_byte_count;
use crate::truncate::truncate_function_output_items_with_policy;
use crate::truncate::truncate_text;
use async_trait::async_trait;
use codex_context_tree::ContextTree;
use codex_context_tree::ContextTreeClient;
use codex_context_tree::ContextTreeNode;
use codex_context_tree::EntryPayload;
use codex_context_tree::EntryRole;
use codex_context_tree::PromptModel;
use codex_context_tree::PromptModelError;
use codex_context_tree::PromptRequest;
use codex_context_tree::PromptResponse;
use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::LocalShellAction;
use codex_protocol::models::LocalShellStatus;
use codex_protocol::models::ReasoningItemContent;
use codex_protocol::models::ReasoningItemReasoningSummary;
use codex_protocol::models::ResponseItem;
use codex_protocol::models::WebSearchAction;
use codex_protocol::protocol::PromptContextActions;
use codex_protocol::protocol::PromptContextNode;
use codex_protocol::protocol::PromptContextTree;
use codex_protocol::protocol::TokenUsage;
use codex_protocol::protocol::TokenUsageInfo;
use futures::StreamExt;
use std::collections::HashSet;
use std::ops::Deref;

/// Transcript of conversation history
#[derive(Debug, Clone)]
pub(crate) struct ContextManager {
    /// The oldest items are at the beginning of the vector.
    items: Vec<ResponseItem>,
    token_info: Option<TokenUsageInfo>,
    tree: ContextTreeState,
    current_task_id: Option<String>,
}

impl ContextManager {
    pub(crate) fn new() -> Self {
        Self::with_context_model(None)
    }

    pub(crate) fn with_context_model(context_model: Option<ModelClient>) -> Self {
        Self {
            items: Vec::new(),
            token_info: TokenUsageInfo::new_or_append(&None, &None, None),
            tree: ContextTreeState::new(context_model),
            current_task_id: None,
        }
    }

    pub(crate) fn token_info(&self) -> Option<TokenUsageInfo> {
        self.token_info.clone()
    }

    pub(crate) fn set_token_info(&mut self, info: Option<TokenUsageInfo>) {
        self.token_info = info;
    }

    pub(crate) fn set_token_usage_full(&mut self, context_window: i64) {
        match &mut self.token_info {
            Some(info) => info.fill_to_context_window(context_window),
            None => {
                self.token_info = Some(TokenUsageInfo::full_context_window(context_window));
            }
        }
    }

    /// `items` is ordered from oldest to newest.
    pub(crate) fn record_items<I>(
        &mut self,
        items: I,
        policy: TruncationPolicy,
        task_id: Option<&str>,
    ) where
        I: IntoIterator,
        I::Item: std::ops::Deref<Target = ResponseItem>,
    {
        self.current_task_id = task_id.map(|id| id.to_string());
        for item in items {
            let item_ref = item.deref();
            let is_ghost_snapshot = matches!(item_ref, ResponseItem::GhostSnapshot { .. });
            if !is_api_message(item_ref) && !is_ghost_snapshot {
                continue;
            }

            let processed = self.process_item(item_ref, policy);
            let index = self.items.len();
            self.tree
                .record_insert(index, &processed, self.current_task_id.as_deref());
            self.items.push(processed);
        }
    }

    pub(crate) fn get_history(&mut self) -> Vec<ResponseItem> {
        self.normalize_history();
        self.contents()
    }

    pub(crate) async fn commit_context_tree(&mut self) {
        self.tree.client.commit_history_mutation().await;
    }

    pub(crate) fn context_tree(&self) -> PromptContextTree {
        build_prompt_context_tree(self.tree.client.cache().tree())
    }

    pub(crate) fn apply_context_actions(&mut self, actions: &[PromptContextActions]) {
        for action in actions {
            if !action.toggle_selected.is_empty() {
                self.tree.client.toggle_selected(&action.toggle_selected);
            }
            if !action.toggle_collapsed.is_empty() {
                self.tree.client.toggle_collapsed(&action.toggle_collapsed);
            }
        }
    }

    // Returns the history prepared for sending to the model.
    // With extra response items filtered out and GhostCommits removed.
    pub(crate) fn get_history_for_prompt(&mut self) -> Vec<ResponseItem> {
        let mut history = self.get_history();
        let selected_ids: HashSet<i64> = self.tree.selected_entry_ids().into_iter().collect();
        if !selected_ids.is_empty() {
            history = history
                .into_iter()
                .enumerate()
                .filter_map(|(idx, item)| {
                    if selected_ids.contains(&(idx as i64)) {
                        Some(item)
                    } else {
                        None
                    }
                })
                .collect();
        }
        Self::remove_ghost_snapshots(&mut history);
        history
    }

    // Estimate token usage using byte-based heuristics from the truncation helpers.
    // This is a coarse lower bound, not a tokenizer-accurate count.
    pub(crate) fn estimate_token_count(&self, turn_context: &TurnContext) -> Option<i64> {
        let model_family = turn_context.client.get_model_family();
        let base_tokens =
            i64::try_from(approx_token_count(model_family.base_instructions.as_str()))
                .unwrap_or(i64::MAX);

        let items_tokens = self.items.iter().fold(0i64, |acc, item| {
            acc + match item {
                ResponseItem::GhostSnapshot { .. } => 0,
                ResponseItem::Reasoning {
                    encrypted_content: Some(content),
                    ..
                }
                | ResponseItem::CompactionSummary {
                    encrypted_content: content,
                } => estimate_reasoning_length(content.len()) as i64,
                item => {
                    let serialized = serde_json::to_string(item).unwrap_or_default();
                    i64::try_from(approx_token_count(&serialized)).unwrap_or(i64::MAX)
                }
            }
        });

        Some(base_tokens.saturating_add(items_tokens))
    }

    pub(crate) fn remove_first_item(&mut self) {
        if !self.items.is_empty() {
            // Remove the oldest item (front of the list). Items are ordered from
            // oldest → newest, so index 0 is the first entry recorded.
            let removed = self.items.remove(0);
            self.tree.record_remove(0);
            // If the removed item participates in a call/output pair, also remove
            // its corresponding counterpart to keep the invariants intact without
            // running a full normalization pass.
            normalize::remove_corresponding_for(&mut self.items, &removed, |index, _| {
                self.tree.record_remove(index);
            });
        }
    }

    pub(crate) fn replace(&mut self, items: Vec<ResponseItem>) {
        let previous_len = self.items.len();
        self.items.clear();
        self.current_task_id = None;
        for _ in 0..previous_len {
            self.tree.record_remove(0);
        }

        for item in items {
            let index = self.items.len();
            self.tree
                .record_insert(index, &item, self.current_task_id.as_deref());
            self.items.push(item);
        }
    }

    pub(crate) fn update_token_info(
        &mut self,
        usage: &TokenUsage,
        model_context_window: Option<i64>,
    ) {
        self.token_info = TokenUsageInfo::new_or_append(
            &self.token_info,
            &Some(usage.clone()),
            model_context_window,
        );
    }

    fn get_non_last_reasoning_items_tokens(&self) -> usize {
        // get reasoning items excluding all the ones after the last user message
        let Some(last_user_index) = self
            .items
            .iter()
            .rposition(|item| matches!(item, ResponseItem::Message { role, .. } if role == "user"))
        else {
            return 0usize;
        };

        let total_reasoning_bytes = self
            .items
            .iter()
            .take(last_user_index)
            .filter_map(|item| {
                if let ResponseItem::Reasoning {
                    encrypted_content: Some(content),
                    ..
                } = item
                {
                    Some(content.len())
                } else {
                    None
                }
            })
            .map(estimate_reasoning_length)
            .fold(0usize, usize::saturating_add);

        let token_estimate = approx_tokens_from_byte_count(total_reasoning_bytes);
        token_estimate as usize
    }

    pub(crate) fn get_total_token_usage(&self) -> i64 {
        self.token_info
            .as_ref()
            .map(|info| info.last_token_usage.total_tokens)
            .unwrap_or(0)
            .saturating_add(self.get_non_last_reasoning_items_tokens() as i64)
    }

    /// This function enforces a couple of invariants on the in-memory history:
    /// 1. every call (function/custom) has a corresponding output entry
    /// 2. every output has a corresponding call entry
    fn normalize_history(&mut self) {
        // all function/tool calls must have a corresponding output
        normalize::ensure_call_outputs_present(&mut self.items, |index, item| {
            self.tree
                .record_insert(index, item, self.current_task_id.as_deref());
        });

        // all outputs must have a corresponding function/tool call
        normalize::remove_orphan_outputs(&mut self.items, |index, _| {
            self.tree.record_remove(index);
        });
    }

    /// Returns a clone of the contents in the transcript.
    fn contents(&self) -> Vec<ResponseItem> {
        self.items.clone()
    }

    fn remove_ghost_snapshots(items: &mut Vec<ResponseItem>) {
        items.retain(|item| !matches!(item, ResponseItem::GhostSnapshot { .. }));
    }

    fn process_item(&self, item: &ResponseItem, policy: TruncationPolicy) -> ResponseItem {
        let policy_with_serialization_budget = policy.mul(1.2);
        match item {
            ResponseItem::FunctionCallOutput { call_id, output } => {
                let truncated =
                    truncate_text(output.content.as_str(), policy_with_serialization_budget);
                let truncated_items = output.content_items.as_ref().map(|items| {
                    truncate_function_output_items_with_policy(
                        items,
                        policy_with_serialization_budget,
                    )
                });
                ResponseItem::FunctionCallOutput {
                    call_id: call_id.clone(),
                    output: FunctionCallOutputPayload {
                        content: truncated,
                        content_items: truncated_items,
                        success: output.success,
                    },
                }
            }
            ResponseItem::CustomToolCallOutput { call_id, output } => {
                let truncated = truncate_text(output, policy_with_serialization_budget);
                ResponseItem::CustomToolCallOutput {
                    call_id: call_id.clone(),
                    output: truncated,
                }
            }
            ResponseItem::Message { .. }
            | ResponseItem::Reasoning { .. }
            | ResponseItem::LocalShellCall { .. }
            | ResponseItem::FunctionCall { .. }
            | ResponseItem::WebSearchCall { .. }
            | ResponseItem::CustomToolCall { .. }
            | ResponseItem::CompactionSummary { .. }
            | ResponseItem::GhostSnapshot { .. }
            | ResponseItem::Other => item.clone(),
        }
    }
}

impl Default for ContextManager {
    fn default() -> Self {
        Self::new()
    }
}

/// API messages include every non-system item (user/assistant messages, reasoning,
/// tool calls, tool outputs, shell calls, and web-search calls).
fn is_api_message(message: &ResponseItem) -> bool {
    match message {
        ResponseItem::Message { role, .. } => role.as_str() != "system",
        ResponseItem::FunctionCallOutput { .. }
        | ResponseItem::FunctionCall { .. }
        | ResponseItem::CustomToolCall { .. }
        | ResponseItem::CustomToolCallOutput { .. }
        | ResponseItem::LocalShellCall { .. }
        | ResponseItem::Reasoning { .. }
        | ResponseItem::WebSearchCall { .. }
        | ResponseItem::CompactionSummary { .. } => true,
        ResponseItem::GhostSnapshot { .. } => false,
        ResponseItem::Other => false,
    }
}

fn estimate_reasoning_length(encoded_len: usize) -> usize {
    encoded_len
        .saturating_mul(3)
        .checked_div(4)
        .unwrap_or(0)
        .saturating_sub(650)
}

#[derive(Debug, Clone)]
struct ContextTreeState {
    client: ContextTreeClient<ContextTreePromptModel>,
}

impl ContextTreeState {
    fn new(context_model: Option<ModelClient>) -> Self {
        Self {
            client: ContextTreeClient::new(context_model.map(ContextTreePromptModel::new)),
        }
    }

    fn record_insert(&mut self, index: usize, item: &ResponseItem, task_id: Option<&str>) {
        let payload = build_entry_payload(item);
        let labels = build_entry_labels(item, task_id);
        self.client.recorder().insert(index, labels, payload);
    }

    fn record_remove(&mut self, index: usize) {
        self.client.recorder().remove(index);
    }

    fn selected_entry_ids(&self) -> Vec<i64> {
        self.client.cache().selected()
    }

    #[cfg(test)]
    fn client(&self) -> &ContextTreeClient<ContextTreePromptModel> {
        &self.client
    }

    #[cfg(test)]
    fn client_mut(&mut self) -> &mut ContextTreeClient<ContextTreePromptModel> {
        &mut self.client
    }
}

#[derive(Debug, Clone)]
struct ContextTreePromptModel {
    client: ModelClient,
}

impl ContextTreePromptModel {
    fn new(client: ModelClient) -> Self {
        Self { client }
    }

    fn build_prompt(&self, request: &PromptRequest) -> Prompt {
        Prompt {
            input: vec![ResponseItem::Message {
                id: None,
                role: "user".to_string(),
                content: vec![ContentItem::InputText {
                    text: request.user_prompt.clone(),
                }],
            }],
            tools: Vec::new(),
            parallel_tool_calls: false,
            base_instructions_override: Some(request.system_prompt.clone()),
            output_schema: None,
        }
    }
}

#[async_trait]
impl PromptModel for ContextTreePromptModel {
    async fn complete(&self, request: PromptRequest) -> Result<PromptResponse, PromptModelError> {
        let prompt = self.build_prompt(&request);

        let mut stream =
            self.client
                .stream(&prompt)
                .await
                .map_err(|err| PromptModelError::Failed {
                    message: err.to_string(),
                })?;

        while let Some(result) = stream.next().await {
            let event = result.map_err(|err| PromptModelError::Failed {
                message: err.to_string(),
            })?;

            if let ResponseEvent::OutputItemDone(item) = event {
                return Ok(PromptResponse {
                    text: entry_body(&item),
                });
            }
        }

        Err(PromptModelError::Failed {
            message: "model stream ended without emitting output".to_string(),
        })
    }
}

fn build_prompt_context_tree(tree: &ContextTree) -> PromptContextTree {
    PromptContextTree {
        root: build_prompt_context_node(&tree.root),
    }
}

fn build_prompt_context_node(node: &ContextTreeNode) -> PromptContextNode {
    let mut children: Vec<PromptContextNode> = node
        .children
        .values()
        .map(build_prompt_context_node)
        .collect();

    children.extend(node.entry_nodes.values().map(build_prompt_context_node));

    PromptContextNode {
        id: node.id.clone(),
        labels: node.labels.clone(),
        title: node.title.clone(),
        summary: node.summary.clone(),
        selected: node.selected,
        collapsed: node.collapsed,
        children,
    }
}

fn build_entry_payload(item: &ResponseItem) -> EntryPayload {
    EntryPayload::new(entry_role(item), entry_title(item), entry_body(item))
}

fn build_entry_labels(item: &ResponseItem, task_id: Option<&str>) -> Vec<String> {
    let mut labels = Vec::new();
    if let Some(task_id) = task_id {
        labels.push(format!("task:{task_id}"));
    }
    match item {
        ResponseItem::Message { role, .. } => {
            labels.push(format!("role:{role}"));
        }
        ResponseItem::Reasoning { .. } => {
            labels.push("assistant".to_string());
            labels.push("reasoning".to_string());
        }
        ResponseItem::FunctionCall { call_id, .. }
        | ResponseItem::FunctionCallOutput { call_id, .. } => {
            labels.push("functions".to_string());
            labels.push(format!("call:{call_id}"));
        }
        ResponseItem::CustomToolCall { call_id, name, .. } => {
            labels.push("custom_tools".to_string());
            labels.push(format!("call:{call_id}"));
            labels.push(format!("tool:{name}"));
        }
        ResponseItem::CustomToolCallOutput { call_id, .. } => {
            labels.push("custom_tools".to_string());
            labels.push(format!("call:{call_id}"));
        }
        ResponseItem::LocalShellCall { call_id, .. } => {
            labels.push("shell".to_string());
            if let Some(id) = call_id {
                labels.push(format!("call:{id}"));
            }
        }
        ResponseItem::WebSearchCall { action, .. } => {
            labels.push("web_search".to_string());
            labels.push(format!("action:{action:?}"));
        }
        ResponseItem::CompactionSummary { .. } => {
            labels.push("compaction".to_string());
        }
        ResponseItem::GhostSnapshot { .. } => {}
        ResponseItem::Other => {
            labels.push("other".to_string());
        }
    }
    labels
}

fn entry_role(item: &ResponseItem) -> EntryRole {
    match item {
        ResponseItem::Message { role, .. } if role == "user" => EntryRole::User,
        ResponseItem::Message { .. } => EntryRole::Assistant,
        ResponseItem::Reasoning { .. } => EntryRole::Assistant,
        ResponseItem::FunctionCall { .. }
        | ResponseItem::FunctionCallOutput { .. }
        | ResponseItem::CustomToolCall { .. }
        | ResponseItem::CustomToolCallOutput { .. }
        | ResponseItem::LocalShellCall { .. }
        | ResponseItem::WebSearchCall { .. } => EntryRole::Tool,
        ResponseItem::CompactionSummary { .. } => EntryRole::Assistant,
        _ => EntryRole::Other,
    }
}

fn entry_title(item: &ResponseItem) -> String {
    match item {
        ResponseItem::Message { role, .. } => format!("{role} message"),
        ResponseItem::Reasoning { .. } => "assistant reasoning".to_string(),
        ResponseItem::FunctionCall { name, .. } => format!("function call: {name}"),
        ResponseItem::FunctionCallOutput { call_id, .. } => {
            format!("function output: {call_id}")
        }
        ResponseItem::CustomToolCall { name, .. } => format!("tool call: {name}"),
        ResponseItem::CustomToolCallOutput { call_id, .. } => {
            format!("tool output: {call_id}")
        }
        ResponseItem::LocalShellCall { .. } => "local shell call".to_string(),
        ResponseItem::WebSearchCall { .. } => "web search".to_string(),
        ResponseItem::CompactionSummary { .. } => "compaction summary".to_string(),
        ResponseItem::GhostSnapshot { .. } => "ghost snapshot".to_string(),
        ResponseItem::Other => "other".to_string(),
    }
}

fn entry_body(item: &ResponseItem) -> String {
    match item {
        ResponseItem::Message { content, .. } => render_content_items(content),
        ResponseItem::Reasoning {
            summary,
            content,
            encrypted_content,
            ..
        } => {
            let mut parts = Vec::new();
            if let Some(text) = render_reasoning_summary(summary) {
                parts.push(text);
            }
            if let Some(content_items) = content {
                parts.push(render_reasoning_content(content_items));
            }
            if let Some(encrypted) = encrypted_content {
                parts.push(format!("encrypted content: {}", encrypted.len()));
            }
            parts.join("\n\n")
        }
        ResponseItem::FunctionCall { arguments, .. } => arguments.clone(),
        ResponseItem::FunctionCallOutput { output, .. } => render_function_output(output),
        ResponseItem::CustomToolCall { input, .. } => input.clone(),
        ResponseItem::CustomToolCallOutput { output, .. } => output.clone(),
        ResponseItem::LocalShellCall { action, status, .. } => {
            describe_local_shell_action(action, Some(status))
        }
        ResponseItem::WebSearchCall { action, .. } => describe_web_search_action(action),
        ResponseItem::CompactionSummary { encrypted_content } => encrypted_content.clone(),
        ResponseItem::GhostSnapshot { .. } => String::new(),
        ResponseItem::Other => String::new(),
    }
}

fn render_content_items(content: &[ContentItem]) -> String {
    let mut parts = Vec::new();
    for item in content {
        match item {
            ContentItem::OutputText { text } | ContentItem::InputText { text } => {
                parts.push(text.clone());
            }
            ContentItem::InputImage { image_url } => {
                parts.push(format!("image: {image_url}"));
            }
        }
    }
    parts.join("\n\n")
}

fn render_reasoning_summary(summary: &[ReasoningItemReasoningSummary]) -> Option<String> {
    let mut summaries = Vec::new();
    for item in summary {
        match item {
            ReasoningItemReasoningSummary::SummaryText { text } => summaries.push(text.clone()),
        }
    }
    if summaries.is_empty() {
        None
    } else {
        Some(summaries.join("\n"))
    }
}

fn render_reasoning_content(content: &[ReasoningItemContent]) -> String {
    let mut parts = Vec::new();
    for item in content {
        match item {
            ReasoningItemContent::ReasoningText { text } => parts.push(text.clone()),
            ReasoningItemContent::Text { text } => parts.push(text.clone()),
        }
    }
    parts.join("\n")
}

fn render_function_output(output: &FunctionCallOutputPayload) -> String {
    let mut body = output.content.clone();
    if let Some(items) = output.content_items.as_ref() {
        let extras = render_function_output_items(items);
        if !extras.is_empty() {
            if !body.is_empty() {
                body.push_str("\n\n");
            }
            body.push_str(&extras);
        }
    }
    body
}

fn render_function_output_items(items: &[FunctionCallOutputContentItem]) -> String {
    let mut parts = Vec::new();
    for item in items {
        match item {
            FunctionCallOutputContentItem::InputText { text } => parts.push(text.clone()),
            FunctionCallOutputContentItem::InputImage { image_url } => {
                parts.push(format!("image: {image_url}"));
            }
        }
    }
    parts.join("\n")
}

fn describe_local_shell_action(
    action: &LocalShellAction,
    status: Option<&LocalShellStatus>,
) -> String {
    match action {
        LocalShellAction::Exec(exec) => {
            let mut segments = vec![exec.command.join(" ")];
            if let Some(user) = &exec.user {
                segments.push(format!("user: {user}"));
            }
            if let Some(dir) = &exec.working_directory {
                segments.push(format!("cwd: {dir}"));
            }
            if let Some(state) = status {
                segments.push(format!("status: {state:?}"));
            }
            segments.join(" | ")
        }
    }
}

fn describe_web_search_action(action: &WebSearchAction) -> String {
    match action {
        WebSearchAction::Search { query } => query.clone().unwrap_or_else(|| "search".to_string()),
        WebSearchAction::OpenPage { url } => url.clone().unwrap_or_default(),
        WebSearchAction::FindInPage { url, pattern } => {
            let mut parts = Vec::new();
            if let Some(link) = url {
                parts.push(link.clone());
            }
            if let Some(pat) = pattern {
                parts.push(pat.clone());
            }
            parts.join(" | ")
        }
        WebSearchAction::Other => "web search action".to_string(),
    }
}

#[cfg(test)]
#[path = "history_tests.rs"]
mod tests;
