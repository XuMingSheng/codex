## Goal

Re-align the forked context handling with upstream by feeding normalized history mutations into `codex-context-tree`, surfacing task-aware trees without protocol leakage, and locking down the documented lifecycle of history commits and context-model ownership.

## Plan

1. Treat the normalized `Vec<ResponseItem>` as the single source of truth, mirroring each mutation into the recorder while letting the tree cache own selection, collapse, and summary state.
2. Keep the context-model provider/model pair in `SessionConfiguration`, spin up session-scoped `ModelClient`s only when summaries need them, and keep `TurnContext` free of dedicated context clients.
3. Capture where history mutations happen, where we commit the recorder (only at turn/task boundaries), and how label chains are derived so future work stays grounded in upstream invariants instead of messy fork history.

## Integration checkpoints

- **History recording:** Every mutation path that touches `self.items` (insert, remove, replace, and the normalization helpers) must feed the recorder the same data so the tree cache’s ids stay deterministic; when you are unsure how normalization should behave, refer back to upstream `core/context_manager`.
- **Task-aware labels:** Core now prefixes every entry with `task:<id>` (dropping the redundant `conversation` label) before appending the type-specific labels (role, functions, tool, etc.), ensuring entries from the same task collapse under the same branch.
- **Commit schedule:** Commit history mutations only at the turn/task boundaries—after seeding the initial conversation and immediately after each task completes or aborts so `/context` can rely on a committed cache before the next prompt. Do not commit just to refresh `/context` mid-task.
- **Context-model ownership:** The base `SessionConfiguration` carries the provider/model pair, and the owning session creates a session-scoped `ModelClient` for the tree summarizer only when requested. `TurnContext` no longer holds a context client, and a missing context model config simply keeps the pathway disabled.
- **Core ↔ crate boundary:** Keep `codex-context-tree` protocol-agnostic. Core translates `ResponseItem`s to `EntryPayload`s and label chains, feeds them to the crate, and only converts the cached tree back into `PromptContextTree` nodes when propagating context snapshots.
- **Selection toggles:** Recorder traffic remains strictly for history mutations. UI selections/collapses are handled by handlers calling the cache’s toggle helpers, so the recorder never emits presentation events or invents ids.

## Integration reminders

- Treat `codex-context-tree` as the arbiter for ids, selection, and collapse state. Translate `ResponseItem`s into `EntryPayload`s/label chains before mutating the recorder, and convert only on the way out to the protocol types.
- Every mutation path should write to both the normalized history vector and the recorder; the tree cache observes these diffs but commits only at task completion or the next prompt so `/context` can tolerate slightly stale reads mid-turn.
- UI-driven selection/collapse toggles belong in the handler. Selections are persisted by calling the cache’s toggle helpers and emitting `Op::UpdatePromptContext`; the recorder should never mirror or invent presentation events.
- Keep the context-model wiring in the session configuration. `TurnContext` no longer owns a context client; `Session::make_context_client` builds a session-scoped `ModelClient` from the base config when summaries request it, and `None` simply disables the pathway.
- When in doubt about normalization invariants, label derivation, or history compaction, look at upstream `core/context_manager`—that hierarchy is the base we’re restoring, not the messy fork history it replaces.

## Core integration snapshot

- `core/src/context_manager/history.rs` now tracks a task id and mirrors every insert/remove/replace into `codex-context-tree`. Label chains start with `task:<id>` and no longer carry the redundant `conversation` prefix; the recorder still emits deterministic mutations so the cache does not rebuild from scratch.
- `ContextManager::record_items` now takes a task identifier, reuses it within the normalization helpers, and only commits the recorder when control is about to return to the user (after seeding history or right after each task finishes/aborts).
- `TurnContext` no longer owns a context model client. The provider/model name stays in `SessionConfiguration`, and the owning session constructs the context `ModelClient` only when a summarization request arrives; a missing config keeps the pathway disabled.
- `Session::commit_context_tree()` runs right after seeding initial history and immediately after each task completes or aborts so `/context` readers see a committed cache before the next prompt.
