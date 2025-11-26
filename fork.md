## Goal

Give users a `/context` command that opens an interactive overlay showing the exact prompt (history items) that will be sent to the model on the next turn. Inside that overlay, users can browse each entry via concise labels, expand entries to inspect full text/tool outputs, and exit back to the main Codex session without advancing the conversation.

## Plan for `/context` interactive prompt viewer

1. **Protocol additions**
   - *Files*: `codex-rs/protocol/src/protocol.rs`
   - *Changes*: add a new `Op::PromptContextRequest` and matching `EventMsg::PromptContextResponse { items: Vec<ResponseItem> }`. Derive schema/TS support so the CLI and TUI can serialize/deserialize the payload.

2. **Core handler to serve prompt context**
   - *Files*: `codex-rs/core/src/codex.rs`
   - *Changes*: extend the submission loop to handle `Op::PromptContextRequest`. The handler will clone the current `ContextManager`, call `get_history_for_prompt()`, and emit the new `PromptContextResponse` event so UIs receive the pending context without advancing the turn.

3. **TUI command plumbing**
   - *Files*: `codex-rs/tui/src/slash_command.rs`, `codex-rs/tui/src/chatwidget.rs`
   - *Changes*: add a `/context` slash command that is always available. Dispatching it should send the new `PromptContextRequest` op and show a loading indicator until the response arrives. When the event arrives, open an overlay instead of appending to the scrollback.

4. **Interactive context overlay**
   - *Files*: new helper module under `codex-rs/tui/src` (e.g., `context_overlay.rs`) plus `app.rs` / `app_event.rs` wiring.
   - *Changes*: implement a fullscreen overlay similar to the existing transcript/diff overlays. The overlay lists each `ResponseItem` with a concise label (role, tool name, etc.), supports navigation/selection keys, and shows the expanded content for the currently highlighted entry. Add an `AppEvent::PromptContext` to deliver the items from the agent to the overlay, and hook Esc/Enter (or a dedicated key) to close the overlay and return to the main Codex UI.

## Discoveries while surveying the codebase

- Slash command definitions live in `codex-rs/tui/src/slash_command.rs`, and dispatching happens in `codex-rs/tui/src/chatwidget.rs`. Any `/context` command must update both spots so the popup description, availability rules, and handler exist.
- Full-screen popups (transcript, `/diff`, etc.) are implemented via `Overlay` in `codex-rs/tui/src/pager_overlay.rs`, with `App` managing an `Option<Overlay>` and routing events through `app_backtrack.rs`. Reusing this machinery will let `/context` open a modal window that captures keyboard input until closed (Esc/Q).
- Backtrack previews already fetch history snapshots through `AppEvent::ConversationHistory` and show them inside the transcript overlay. That pattern (request data, receive via `AppEvent`, render overlay) can be applied to prompt-context viewing, possibly with a dedicated overlay variant if UX requirements differ from the transcript view.
