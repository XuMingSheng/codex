use std::collections::HashSet;

use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ResponseItem;

use crate::context_manager::history::ContextEntry;
use crate::util::error_or_panic;

pub(crate) fn ensure_call_outputs_present(items: &mut Vec<ContextEntry>, next_id: &mut i64) {
    // Collect synthetic outputs to insert immediately after their calls.
    // Store the insertion position (index of call) alongside the item so
    // we can insert in reverse order and avoid index shifting.
    let mut missing_outputs_to_insert: Vec<(usize, ResponseItem)> = Vec::new();

    for (idx, entry) in items.iter().enumerate() {
        match &entry.item {
            ResponseItem::FunctionCall { call_id, .. } => {
                let has_output = items.iter().any(|i| match &i.item {
                    ResponseItem::FunctionCallOutput {
                        call_id: existing, ..
                    } => existing == call_id,
                    _ => false,
                });

                if !has_output {
                    error_or_panic(format!(
                        "Function call output is missing for call id: {call_id}"
                    ));
                    missing_outputs_to_insert.push((
                        idx,
                        ResponseItem::FunctionCallOutput {
                            call_id: call_id.clone(),
                            output: FunctionCallOutputPayload {
                                content: "aborted".to_string(),
                                ..Default::default()
                            },
                        },
                    ));
                }
            }
            ResponseItem::CustomToolCall { call_id, .. } => {
                let has_output = items.iter().any(|i| match &i.item {
                    ResponseItem::CustomToolCallOutput {
                        call_id: existing, ..
                    } => existing == call_id,
                    _ => false,
                });

                if !has_output {
                    error_or_panic(format!(
                        "Custom tool call output is missing for call id: {call_id}"
                    ));
                    missing_outputs_to_insert.push((
                        idx,
                        ResponseItem::CustomToolCallOutput {
                            call_id: call_id.clone(),
                            output: "aborted".to_string(),
                        },
                    ));
                }
            }
            // LocalShellCall is represented in upstream streams by a FunctionCallOutput
            ResponseItem::LocalShellCall { call_id, .. } => {
                if let Some(call_id) = call_id.as_ref() {
                    let has_output = items.iter().any(|i| match &i.item {
                        ResponseItem::FunctionCallOutput {
                            call_id: existing, ..
                        } => existing == call_id,
                        _ => false,
                    });

                    if !has_output {
                        error_or_panic(format!(
                            "Local shell call output is missing for call id: {call_id}"
                        ));
                        missing_outputs_to_insert.push((
                            idx,
                            ResponseItem::FunctionCallOutput {
                                call_id: call_id.clone(),
                                output: FunctionCallOutputPayload {
                                    content: "aborted".to_string(),
                                    ..Default::default()
                                },
                            },
                        ));
                    }
                }
            }
            _ => {}
        }
    }

    // Insert synthetic outputs in reverse index order to avoid re-indexing.
    for (idx, output_item) in missing_outputs_to_insert.into_iter().rev() {
        let id = *next_id;
        *next_id += 1;
        items.insert(
            idx + 1,
            ContextEntry {
                id,
                selected: true,
                item: output_item,
            },
        );
    }
}

pub(crate) fn remove_orphan_outputs(items: &mut Vec<ContextEntry>) {
    let function_call_ids: HashSet<String> = items
        .iter()
        .filter_map(|i| match &i.item {
            ResponseItem::FunctionCall { call_id, .. } => Some(call_id.clone()),
            _ => None,
        })
        .collect();

    let local_shell_call_ids: HashSet<String> = items
        .iter()
        .filter_map(|i| match &i.item {
            ResponseItem::LocalShellCall {
                call_id: Some(call_id),
                ..
            } => Some(call_id.clone()),
            _ => None,
        })
        .collect();

    let custom_tool_call_ids: HashSet<String> = items
        .iter()
        .filter_map(|i| match &i.item {
            ResponseItem::CustomToolCall { call_id, .. } => Some(call_id.clone()),
            _ => None,
        })
        .collect();

    items.retain(|entry| match &entry.item {
        ResponseItem::FunctionCallOutput { call_id, .. } => {
            let call_id_ref = call_id.as_str();
            let has_match = function_call_ids.contains(call_id_ref)
                || local_shell_call_ids.contains(call_id_ref);
            if !has_match {
                error_or_panic(format!(
                    "Orphan function call output for call id: {call_id}"
                ));
            }
            has_match
        }
        ResponseItem::CustomToolCallOutput { call_id, .. } => {
            let has_match = custom_tool_call_ids.contains(call_id.as_str());
            if !has_match {
                error_or_panic(format!(
                    "Orphan custom tool call output for call id: {call_id}"
                ));
            }
            has_match
        }
        _ => true,
    });
}

pub(crate) fn remove_corresponding_for(items: &mut Vec<ContextEntry>, item: &ResponseItem) {
    match item {
        ResponseItem::FunctionCall { call_id, .. } => {
            remove_first_matching(items, |item| {
                matches!(
                    item,
                    ResponseItem::FunctionCallOutput {
                        call_id: existing,
                        ..
                    } if existing == call_id
                )
            });
        }
        ResponseItem::FunctionCallOutput { call_id, .. } => {
            if let Some(pos) = items.iter().position(|entry| {
                matches!(
                    &entry.item,
                    ResponseItem::FunctionCall { call_id: existing, .. } if existing == call_id
                )
            }) {
                items.remove(pos);
            } else if let Some(pos) = items.iter().position(|entry| {
                matches!(
                    &entry.item,
                    ResponseItem::LocalShellCall {
                        call_id: Some(existing),
                        ..
                    } if existing == call_id
                )
            }) {
                items.remove(pos);
            }
        }
        ResponseItem::CustomToolCall { call_id, .. } => {
            remove_first_matching(items, |item| {
                matches!(
                    item,
                    ResponseItem::CustomToolCallOutput {
                        call_id: existing,
                        ..
                    } if existing == call_id
                )
            });
        }
        ResponseItem::CustomToolCallOutput { call_id, .. } => {
            remove_first_matching(items, |item| {
                matches!(
                    item,
                    ResponseItem::CustomToolCall { call_id: existing, .. } if existing == call_id
                )
            });
        }
        ResponseItem::LocalShellCall {
            call_id: Some(call_id),
            ..
        } => {
            remove_first_matching(items, |item| {
                matches!(
                    item,
                    ResponseItem::FunctionCallOutput {
                        call_id: existing,
                        ..
                    } if existing == call_id
                )
            });
        }
        _ => {}
    }
}

fn remove_first_matching<F>(items: &mut Vec<ContextEntry>, predicate: F)
where
    F: Fn(&ResponseItem) -> bool,
{
    if let Some(pos) = items.iter().position(|entry| predicate(&entry.item)) {
        items.remove(pos);
    }
}
