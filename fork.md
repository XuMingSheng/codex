## Goal

Make the `/context` overlay interactive so users can choose which history items will be sent to the model on the next turn. Each entry in the prompt context needs a selectable toggle, only the checked items should be included in the next prompt, and the user’s choices must persist across turns (newly added items default to selected until the user changes them).

## Plan for selectable prompt context

1. **Protocol extensions**
   - *Files*: `codex-rs/protocol/src/protocol.rs`
   - *Changes*: add stable identifiers plus selection metadata to prompt-context payloads (e.g., `PromptContextItem` with `id`, `selected`, and `ResponseItem`). Introduce a new `PromptContextUpdate` op carrying selection deltas so UIs can persist changes. Update schemas/TS bindings accordingly.

2. **Core context manager & filtering**
   - *Files*: `codex-rs/core/src/context_manager/**`, `codex-rs/core/src/codex.rs`
   - *Changes*: teach `ContextManager` to track per-item selection status (default true for new items), expose the flag through `get_history_for_prompt`, and respect it when building the turn input. Handle incoming `PromptContextUpdate` ops by flipping the stored selection bits, and ensure the selection survives compaction/backtrack flows.

3. **TUI overlay interactions**
   - *Files*: `codex-rs/tui/src/context_overlay.rs`, `app.rs`, `app_event.rs`, `chatwidget.rs`
   - *Changes*: show a checkbox or similar affordance next to each entry, allow toggling with keyboard shortcuts (Space/Enter), show selected counts, and send `PromptContextUpdate` ops whenever the user changes a selection. Persist state so reopening `/context` reflects prior choices, and make sure new history items appear as selected.

4. **Docs and regression coverage**
   - *Files*: `docs/slash_commands.md`, relevant tests in `codex-rs/core` and/or snapshot coverage in `codex-rs/tui`
   - *Changes*: document the selection behavior, add unit tests for selection persistence/filtering, and refresh any UI snapshots touched by the overlay updates.

## Discoveries while surveying the codebase

- Slash command definitions live in `codex-rs/tui/src/slash_command.rs`, and dispatching happens in `codex-rs/tui/src/chatwidget.rs`. Any `/context` command must update both spots so the popup description, availability rules, and handler exist.
- Full-screen popups (transcript, `/diff`, etc.) are implemented via `Overlay` in `codex-rs/tui/src/pager_overlay.rs`, with `App` managing an `Option<Overlay>` and routing events through `app_backtrack.rs`. Reusing this machinery will let `/context` open a modal window that captures keyboard input until closed (Esc/Q).
- Backtrack previews already fetch history snapshots through `AppEvent::ConversationHistory` and show them inside the transcript overlay. That pattern (request data, receive via `AppEvent`, render overlay) can be applied to prompt-context viewing, possibly with a dedicated overlay variant if UX requirements differ from the transcript view.

## Implementation summary

- **Protocol/core plumbing** (`codex-rs/protocol/src/protocol.rs`, `codex-rs/core/src/codex.rs`, `codex-rs/core/src/rollout/policy.rs`): added `Op::PromptContextRequest` and `EventMsg::PromptContextResponse`, wired the submission loop to clone the `ContextManager` and emit the new event, and marked the event as non-persisted in rollout files.
- **TUI command & overlay** (`codex-rs/tui/src/slash_command.rs`, `codex-rs/tui/src/chatwidget.rs`, `codex-rs/tui/src/app.rs`, `codex-rs/tui/src/app_event.rs`, `codex-rs/tui/src/pager_overlay.rs`, new `codex-rs/tui/src/context_overlay.rs`): introduced the `/context` slash command, forwarded the response via a new app event, and rendered it through a dedicated overlay that lists prompt items on the left and shows expanded content on the right with scroll/navigation controls.
- **Exec/MCP fallthrough** (`codex-rs/exec/src/event_processor_with_human_output.rs`, `codex-rs/mcp-server/src/codex_tool_runner.rs`): treated `PromptContextResponse` like other no-op events so their matches stay exhaustive.
- **Docs** (`docs/slash_commands.md`): documented `/context` alongside the other built-in commands.
