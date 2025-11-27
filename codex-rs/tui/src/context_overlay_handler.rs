use crate::chatwidget::ChatWidget;
use crate::context_overlay::ContextOverlay;
use crate::context_overlay::ExitAction;
use crate::tui;
use codex_core::protocol::Op;

pub(crate) fn handle_close(ctx: &mut ContextOverlay, chat_widget: &mut ChatWidget) {
    match ctx.exit_action() {
        ExitAction::Save => {
            let updates = ctx.take_selection_updates();
            if !updates.is_empty() {
                chat_widget.submit_op(Op::UpdatePromptContextSelection {
                    selections: updates,
                });
            }
        }
        ExitAction::Cancel => {
            ctx.take_selection_updates();
        }
    }
}

pub(crate) fn open_context_overlay(
    overlay: &mut Option<crate::pager_overlay::Overlay>,
    tui: &mut tui::Tui,
    items: Vec<codex_protocol::protocol::PromptContextItem>,
) {
    if overlay.is_some() {
        let _ = tui.leave_alt_screen();
    }
    let _ = tui.enter_alt_screen();
    *overlay = Some(crate::pager_overlay::Overlay::new_context(items));
    tui.frame_requester().schedule_frame();
}
