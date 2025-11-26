## Current refactor goal

Separate the `/context` overlay logic from backtracking logic. Only the shared, feature-neutral overlay plumbing (draw/key dispatch, close detection) should live in a neutral router. Backtrack rollback logic stays in backtrack modules; `/context` save/cancel and prompt-context updates stay in context modules.

## Refactor plan for overlay separation

1) Extract a neutral overlay router module
   - APIs: `forward_overlay_event(&mut Option<Overlay>, &mut Tui, TuiEvent) -> Result<bool>` (generic close detection) and `close_overlay(&mut Tui, &mut Option<Overlay>)` (alt-screen teardown only).
   - No knowledge of backtrack or context; no branching on `Overlay` variants beyond `is_done` and forwarding `handle_event`.

2) Keep backtrack-specific behavior isolated
   - `app_backtrack.rs` retains preview/highlight, rollback confirmation, Esc/Enter handling, and transcript overlay state.
   - Backtrack handles overlay close results that matter to rollback only; no context awareness.

3) Keep `/context` behavior isolated
   - `context_overlay.rs` handles save/cancel keys and stores pending `PromptContextSelection` updates.
   - A small context handler (new file if needed) consumes close events and emits `Op::PromptContextUpdate` on save; no backtrack logic.

4) Wire `App` to the neutral router
   - `app.rs` calls the router for generic event forwarding/close detection, then delegates to either backtrack code or context handler based on the active overlay variant.
   - Remove context-specific update sending from `app_backtrack.rs`; remove backtrack logic from context paths.
