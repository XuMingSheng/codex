## Context tree architecture

The context-tree crate now splits responsibilities across four pieces: the mutation recorder, an incremental cache + tree structure, a summarizer, and the public client wrapper. This section documents the current flow so future work stays aligned.

1. **History mutation recorder**
   - `HistoryMutationRecorder::new(history_len)` seeds a `MutatedEntry::Mirror { original_index }` for every slot that already exists in history.
   - Core calls `insert/replace/remove` as it mutates the normalized history vector. Inserts capture the label chain and the textual payload (role/title/body) required for summaries; a replace is implemented as `remove + insert` at the same index.
   - When core wants to synchronize with the cache it calls `commit()`. The recorder diffs its mirror against the pending list and emits a flat `Vec<HistoryMutation>` containing:
     * `IndexChange { old_index, new_index }` whenever an original entry moved to a new index.
     * `Remove { index }` for each original entry that disappeared.
     * `Insert { index, labels, payload }` for every new entry in the pending list.
   - After emitting the mutations the recorder rebuilds its mirror (now perfectly aligned with the latest history order) so the next round of edits starts from a clean slate.

2. **Cache + tree maintenance**
   - `ContextTreeCache` owns two pieces of state:
     * `BTreeMap<i64, CachedEntry>` keyed by history index (ids always equal indices). This keeps entry metadata accessible by id and guarantees chronological iteration by key order.
     * `ContextTree`, which is a nested hierarchy of `ContextTreeNode`s. Each group node is stored inside a `BTreeMap<label, ContextTreeNode>` so children are deterministically ordered by label. Leaves live in `entry_nodes` maps with ids of the form `entry-{id}`. Every node carries `selected`, `collapsed`, and `updated` bits so UI state and summarization work stays local to the mutated subtrees.
   - `apply_mutations(summarizer, mutations)` processes a batch in three passes:
     1. **Removes** – entries are dropped from the `BTreeMap`, and `ContextTree::remove_entry_node` prunes the corresponding leaves (recursively removing empty label nodes). Those nodes are marked `updated = true` so summaries for the affected branches get recomputed.
     2. **Index changes** – entries stay in the cache but their ids (keys) are remapped. The tree renames matching entry nodes via `rename_entry_nodes`, keeping the label structure intact while reusing existing summaries/selection bits.
     3. **Inserts** – brand-new `CachedEntry`s are pushed into the map and `ContextTree::insert_entry_node` walks their label chain, creating missing intermediate nodes on the fly. Newly created nodes start selected/expanded.
   - After all mutations are applied, the cache hands the tree + entries map to the summarizer. Because every structural change toggles `updated`, the summarizer only revisits nodes that changed instead of rebuilding entire subtrees.
   - Selection/collapse updates operate directly on the tree via `toggle_selected`/`toggle_collapsed`, keeping presentation state co-located with the nodes.

3. **Summarizer**
   - `ContextTreeSummarizer` receives the shared tree and entry map plus an optional `PromptModel`. When a `ModelClient` is configured in the session, `ContextManager` wraps it inside `ContextTreePromptModel` so the summarizer can call the real context model; otherwise it relies on the combined text that the cache already assembled.
   - `summarize_tree` recurses through the tree. When a node’s `updated` flag is `false` the summarizer leaves it alone. Otherwise it:
     * Recursively summarises child label nodes.
     * Summarises each entry leaf by clipping the cached entry title/body.
     * Concatenates child summaries to form the parent node summary. When a context `ModelClient` is available, `ContextTreePromptModel` builds a prompt with the configured system instructions and the concatenated text, streams it through the model, and uses the assistant’s final message as the new summary. Any streaming error or premature end simply causes the summarizer to fall back to the combined text so the cache never loses a usable summary.
   - Once a node is processed its `updated` flag resets to `false`, so later commits only touch nodes that truly changed.

4. **Client surface**
- `ContextTreeClient` bundles the mutation recorder, cache, and summarizer. Core mutates the recorder directly (`insert/replace/remove`) and calls `commit_history_mutation()` when it needs the cache/tree to catch up.
- `commit_history_mutation()` drains the recorder (via `commit()`), feeds the mutations into `ContextTreeCache::apply_mutations`, and awaits summarization. Core can then read the cached tree or selected ids through the cache accessor.

This design keeps ids stable (they always match the history index), updates only the branches touched by each mutation batch, and avoids rebuilding trees from scratch. The BTree-backed structure guarantees deterministic ordering, while the incremental `updated` bit confines summarization work to the nodes that really changed.

### Label conventions

Core now emits deterministic label chains that begin with the task identifier (`task:<id>`) and no longer carry the redundant `conversation` prefix. Entries that belong in the same task must share that prefix so `ContextTree::insert_entry_node` routes them into the correct branch before the type-specific labels (role, functions, custom tools, etc.) extend the path. Any time a label chain changes, emit the matching remove/insert mutations so the cache recreates the branch cleanly.

## Integration reminders

- Treat `codex-context-tree` as the single source of truth for ids, selection, collapse state, and the new task-prefixed label chains. Core translates `ResponseItem`s into `EntryPayload`s/label chains, hands them to the crate, and lets the cache manage summaries/selection; UI-driven selection/collapse toggles call the cache directly so the recorder never emits presentation events.
- Keep the crate protocol-free: `PromptContext*` structs stay in `codex_protocol`, while the tree/cache operate on their own node types. Convert the cached tree back into protocol types only when sending context snapshots out.
- Record every mutation where it actually happens (`record_items`, compaction helpers, normalization, etc.), and only commit the recorder at turn/task boundaries—after the initial history seed and immediately before control returns to the user, right after each task completes or aborts. `/context` reads may see slightly stale caches between commits, so avoid committing for every overlay open; when uncertain about normalization or labels, consult upstream `core/context_manager` for the invariants we are mirroring.
- The session configuration continues to own the context-model provider and model name; `TurnContext` no longer carries a dedicated context client. Build a session-scoped `ModelClient` from that config and hand it to `ContextManager`/the tree only when summaries are requested so the rest of the pipeline can stay agnostic about context-model lifetimes.

## Core integration snapshot

- `core/src/context_manager/history.rs` now mirrors every mutation into the recorder with the current task id, the recorder no longer emits the `conversation` label, and commits only occur at turn/task boundaries (after the initial seed and immediately before the user sees the next prompt) so `/context` reads can tolerate slightly stale caches.
- Selection/collapse toggles remain UI responsibilities; handlers call the cache’s toggle APIs directly. The recorder never emits these actions, and it does not invent ids or selection bits—if the mutation stream mirrors history precisely, the cache stays in sync without extra bookkeeping.
- `TurnContext` no longer carries a context model client. The provider/model live in `SessionConfiguration`, so `ContextManager` (or its owning `SessionState`) should create a session-scoped `ModelClient` exactly when the tree summarizer needs it, keeping every other turn agile.
- `Session` now owns a dedicated `tokio::Mutex<ContextManager>` seeded with `ContextManager::with_context_model(make_context_client(...))`. `Session::commit_context_tree()` runs after we seed initial history and immediately after each task completes or aborts, guaranteeing `/context` uses a committed cache before the next prompt.
- The context-model summarizer runs through `SessionSource::SubAgent(SubAgentSource::Other("context-tree"))`, so the SSE stream that drives `ContextTreePromptModel` never propagates into the session’s history or recorder.
