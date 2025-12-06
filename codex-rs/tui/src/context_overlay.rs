use crate::key_hint;
use crate::key_hint::KeyBinding;
use crate::tui;
use crate::tui::TuiEvent;
use crate::wrapping::RtOptions;
use crate::wrapping::word_wrap_lines;
use codex_protocol::protocol::PromptContextActions;
use codex_protocol::protocol::PromptContextNode;
use codex_protocol::protocol::PromptContextTree;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use ratatui::buffer::Buffer;
use ratatui::layout::Constraint;
use ratatui::layout::Direction;
use ratatui::layout::Layout;
use ratatui::layout::Rect;
use ratatui::prelude::Widget;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Clear;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Wrap;
use std::io::Result;
use textwrap::Options;

const KEY_UP: KeyBinding = key_hint::plain(KeyCode::Up);
const KEY_DOWN: KeyBinding = key_hint::plain(KeyCode::Down);
const KEY_J: KeyBinding = key_hint::plain(KeyCode::Char('j'));
const KEY_K: KeyBinding = key_hint::plain(KeyCode::Char('k'));
const KEY_PAGE_UP: KeyBinding = key_hint::plain(KeyCode::PageUp);
const KEY_PAGE_DOWN: KeyBinding = key_hint::plain(KeyCode::PageDown);
const KEY_LEFT_ARROW: KeyBinding = key_hint::plain(KeyCode::Left);
const KEY_RIGHT_ARROW: KeyBinding = key_hint::plain(KeyCode::Right);
const KEY_H_LOWER: KeyBinding = key_hint::plain(KeyCode::Char('h'));
const KEY_L_LOWER: KeyBinding = key_hint::plain(KeyCode::Char('l'));
const KEY_H_UPPER: KeyBinding = key_hint::plain(KeyCode::Char('H'));
const KEY_L_UPPER: KeyBinding = key_hint::plain(KeyCode::Char('L'));
const KEY_SPACE: KeyBinding = key_hint::plain(KeyCode::Char(' '));
const KEY_ENTER: KeyBinding = key_hint::plain(KeyCode::Enter);
const KEY_SAVE: KeyBinding = key_hint::plain(KeyCode::Char('s'));
const KEY_CANCEL: KeyBinding = key_hint::plain(KeyCode::Char('x'));
const KEY_CLOSE_Q: KeyBinding = key_hint::plain(KeyCode::Char('q'));
const KEY_CLOSE_ESC: KeyBinding = key_hint::plain(KeyCode::Esc);

#[derive(Clone)]
struct OverlayNode {
    id: String,
    labels: Vec<String>,
    title: String,
    summary: String,
    selected: bool,
    collapsed: bool,
    children: Vec<OverlayNode>,
    min_entry_id: Option<i64>,
}

impl OverlayNode {
    fn from_prompt(node: &PromptContextNode) -> Self {
        let mut children: Vec<OverlayNode> =
            node.children.iter().map(OverlayNode::from_prompt).collect();
        children.sort_by_key(|child| child.min_entry_id.unwrap_or(i64::MAX));
        let min_entry_id = if node.id.starts_with("entry-") {
            node.id
                .strip_prefix("entry-")
                .and_then(|id| id.parse::<i64>().ok())
        } else {
            children.iter().filter_map(|child| child.min_entry_id).min()
        };

        Self {
            id: node.id.clone(),
            labels: node.labels.clone(),
            title: node.title.clone(),
            summary: node.summary.clone(),
            selected: node.selected,
            collapsed: node.collapsed,
            children,
            min_entry_id,
        }
    }
}

pub(crate) struct ContextOverlay {
    root: OverlayNode,
    visible_paths: Vec<Vec<usize>>,
    selected: usize,
    list_scroll: usize,
    detail_scroll: usize,
    last_detail_total_height: usize,
    last_detail_area_height: u16,
    is_done: bool,
    exit_action: Option<ExitAction>,
    pending_selected: Vec<String>,
    pending_collapsed: Vec<String>,
}

impl ContextOverlay {
    pub(crate) fn new(tree: Option<PromptContextTree>) -> Self {
        let root = tree
            .as_ref()
            .map(|ctx| OverlayNode::from_prompt(&ctx.root))
            .unwrap_or_else(Self::placeholder_root);
        let mut overlay = Self {
            root,
            visible_paths: Vec::new(),
            selected: 0,
            list_scroll: 0,
            detail_scroll: 0,
            last_detail_total_height: 0,
            last_detail_area_height: 0,
            is_done: false,
            exit_action: None,
            pending_selected: Vec::new(),
            pending_collapsed: Vec::new(),
        };
        overlay.rebuild_visible_paths();
        overlay
    }

    fn placeholder_root() -> OverlayNode {
        let child = OverlayNode {
            id: "placeholder-child".to_string(),
            labels: vec!["conversation".to_string()],
            title: "Context tree unavailable".to_string(),
            summary: "No prompt context tree is available right now.".to_string(),
            selected: true,
            collapsed: false,
            children: Vec::new(),
            min_entry_id: None,
        };
        OverlayNode {
            id: "placeholder-root".to_string(),
            labels: vec!["conversation".to_string()],
            title: "Conversation".to_string(),
            summary: String::new(),
            selected: true,
            collapsed: false,
            children: vec![child],
            min_entry_id: None,
        }
    }

    pub(crate) fn handle_event(&mut self, tui: &mut tui::Tui, event: TuiEvent) -> Result<()> {
        match event {
            TuiEvent::Draw => {
                tui.draw(u16::MAX, |frame| {
                    self.render(frame.area(), frame.buffer);
                })?;
            }
            TuiEvent::Key(key_event) => {
                if self.handle_key(key_event) {
                    tui.frame_requester().schedule_frame();
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub(crate) fn is_done(&self) -> bool {
        self.is_done
    }

    pub(crate) fn exit_action(&self) -> ExitAction {
        self.exit_action.unwrap_or(ExitAction::Cancel)
    }

    pub(crate) fn take_selection_updates(&mut self) -> PromptContextActions {
        PromptContextActions {
            toggle_selected: std::mem::take(&mut self.pending_selected),
            toggle_collapsed: std::mem::take(&mut self.pending_collapsed),
        }
    }

    pub(crate) fn discard_selection_updates(&mut self) {
        self.pending_selected.clear();
        self.pending_collapsed.clear();
    }

    fn handle_key(&mut self, key_event: KeyEvent) -> bool {
        if key_event.kind != KeyEventKind::Press && key_event.kind != KeyEventKind::Repeat {
            return false;
        }
        match key_event.code {
            KeyCode::Char('q') | KeyCode::Esc => self.cancel_exit(),
            KeyCode::Char('x') => self.cancel_exit(),
            KeyCode::Char('s') => self.save_and_exit(),
            KeyCode::Char(' ') | KeyCode::Enter => self.toggle_selection(),
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::PageUp => self.scroll_detail(-(self.page_scroll_amount() as isize)),
            KeyCode::PageDown => self.scroll_detail(self.page_scroll_amount() as isize),
            KeyCode::Left | KeyCode::Char('h') => self.collapse_current(false),
            KeyCode::Right | KeyCode::Char('l') => self.expand_current(false),
            KeyCode::Char('H') => self.collapse_current(true),
            KeyCode::Char('L') => self.expand_current(true),
            _ => false,
        }
    }

    fn page_scroll_amount(&self) -> usize {
        self.last_detail_area_height.max(1) as usize
    }

    fn move_selection(&mut self, delta: isize) -> bool {
        if self.visible_paths.is_empty() {
            return false;
        }
        let len = self.visible_paths.len() as isize;
        let mut next = self.selected as isize + delta;
        if next < 0 {
            next = 0;
        } else if next >= len {
            next = len - 1;
        }
        let changed = next as usize != self.selected;
        self.selected = next as usize;
        if changed {
            self.detail_scroll = 0;
        }
        changed
    }

    fn scroll_detail(&mut self, delta: isize) -> bool {
        if delta == 0 || self.last_detail_total_height == 0 {
            return false;
        }
        let current = self.detail_scroll as isize;
        let mut next = current + delta;
        if next < 0 {
            next = 0;
        }
        if next as usize == self.detail_scroll {
            return false;
        }
        self.detail_scroll = next as usize;
        true
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer) {
        Clear.render(area, buf);
        if area.width == 0 || area.height == 0 {
            return;
        }
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(3), Constraint::Length(2)].as_ref())
            .split(area);
        self.render_main(chunks[0], buf);
        self.render_hints(chunks[1], buf);
    }

    fn render_main(&mut self, area: Rect, buf: &mut Buffer) {
        let min_detail = 20;
        let default_left = area.width / 3;
        let mut left_width = default_left.max(20);
        if area.width.saturating_sub(left_width) < min_detail {
            left_width = area.width.saturating_sub(min_detail).max(10);
        }
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(left_width), Constraint::Min(min_detail)].as_ref())
            .split(area);
        self.render_list(chunks[0], buf);
        self.render_divider(area, chunks[1], buf);
        self.render_detail(chunks[1], buf);
    }

    fn render_divider(&self, area: Rect, right: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 || right.width == 0 {
            return;
        }
        let divider_x = right.x.saturating_sub(1);
        if divider_x < area.x || divider_x >= buf.area().width {
            return;
        }
        let style = Style::default().fg(Color::DarkGray);
        for y in area.y..area.y + area.height {
            if y >= buf.area().height {
                break;
            }
            if let Some(cell) = buf.cell_mut((divider_x, y)) {
                cell.set_char('│').set_style(style);
            }
        }
    }

    fn render_list(&mut self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        self.ensure_selection_visible(area.height as usize);
        let visible = area.height as usize;
        let mut lines = Vec::with_capacity(visible);
        for (idx, path) in self
            .visible_paths
            .iter()
            .enumerate()
            .skip(self.list_scroll)
            .take(visible)
        {
            let Some(node) = self.node(path) else {
                continue;
            };
            let indent = "  ".repeat(path.len().saturating_sub(1));
            let marker = selection_marker(selection_state(node));
            let fold = if node.children.is_empty() {
                "  "
            } else if node.collapsed {
                "> "
            } else {
                "v "
            };
            let prefix = format!("{indent}{fold}{marker} ");
            let subsequent_indent = " ".repeat(prefix.len());
            let opts = Options::new(area.width.max(1) as usize)
                .initial_indent(&prefix)
                .subsequent_indent(&subsequent_indent);
            let wrapped = textwrap::wrap(&node.title, &opts)
                .into_iter()
                .map(std::borrow::Cow::into_owned)
                .collect::<Vec<_>>();
            let lines_for_node = if wrapped.is_empty() {
                vec![prefix.clone()]
            } else {
                wrapped
            };
            for line in lines_for_node {
                let text = if idx == self.selected {
                    Line::from(line.cyan().bold())
                } else {
                    Line::from(line)
                };
                lines.push(text);
            }
        }
        if lines.is_empty() {
            lines.push(Line::from("No nodes available".dim()));
        }
        Paragraph::new(lines).render(area, buf);
    }

    fn ensure_selection_visible(&mut self, visible: usize) {
        if visible == 0 || self.visible_paths.is_empty() {
            self.list_scroll = 0;
            return;
        }
        if self.selected < self.list_scroll {
            self.list_scroll = self.selected;
        } else if self.selected >= self.list_scroll + visible {
            self.list_scroll = self.selected + 1 - visible;
        }
    }

    fn render_detail(&mut self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            self.last_detail_area_height = 0;
            self.last_detail_total_height = 0;
            return;
        }
        let lines = if let Some(path) = self.visible_paths.get(self.selected) {
            if let Some(node) = self.node(path) {
                detail_lines_for_node(node)
            } else {
                vec![Line::from("No details available.".dim())]
            }
        } else {
            vec![Line::from("No details available.".dim())]
        };
        let wrapped = word_wrap_lines(lines, RtOptions::new(area.width as usize));
        self.last_detail_total_height = wrapped.len();
        self.last_detail_area_height = area.height;
        let max_scroll = self
            .last_detail_total_height
            .saturating_sub(area.height as usize);
        if max_scroll == 0 {
            self.detail_scroll = 0;
        } else {
            self.detail_scroll = self.detail_scroll.min(max_scroll);
        }
        Paragraph::new(wrapped)
            .wrap(Wrap { trim: false })
            .scroll((self.detail_scroll as u16, 0))
            .render(area, buf);
    }

    fn render_hints(&self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 {
            return;
        }
        let mut spans: Vec<Span<'static>> = vec![];
        spans.extend(render_hint_segment(
            &[KEY_UP, KEY_DOWN, KEY_J, KEY_K],
            "navigate",
        ));
        spans.extend(render_hint_segment(
            &[KEY_PAGE_UP, KEY_PAGE_DOWN],
            "scroll detail",
        ));
        spans.extend(render_hint_segment(&[KEY_SPACE, KEY_ENTER], "toggle"));
        spans.extend(render_hint_segment(
            &[KEY_LEFT_ARROW, KEY_H_LOWER],
            "collapse",
        ));
        spans.extend(render_hint_segment(
            &[KEY_RIGHT_ARROW, KEY_L_LOWER],
            "expand",
        ));
        spans.extend(render_hint_segment(&[KEY_H_UPPER], "collapse all"));
        spans.extend(render_hint_segment(&[KEY_L_UPPER], "expand all"));
        spans.extend(render_hint_segment(&[KEY_SAVE], "save & exit"));
        spans.extend(render_hint_segment(
            &[KEY_CLOSE_Q, KEY_CLOSE_ESC, KEY_CANCEL],
            "cancel",
        ));
        Paragraph::new(Line::from(spans)).render(area, buf);
    }

    fn toggle_selection(&mut self) -> bool {
        let Some(path) = self.visible_paths.get(self.selected).cloned() else {
            return false;
        };
        let mut changed_nodes = Vec::new();
        {
            let Some(node) = self.node_mut(&path) else {
                return false;
            };
            let new_state = !node.selected;
            node.set_descendants_selected(new_state, &mut changed_nodes);
        }
        if changed_nodes.is_empty() {
            return false;
        }
        self.pending_selected.extend(changed_nodes);
        true
    }

    fn collapse_current(&mut self, recursive: bool) -> bool {
        self.set_collapse_state(true, recursive)
    }

    fn expand_current(&mut self, recursive: bool) -> bool {
        self.set_collapse_state(false, recursive)
    }

    fn set_collapse_state(&mut self, collapse: bool, recursive: bool) -> bool {
        let Some(path) = self.visible_paths.get(self.selected).cloned() else {
            return false;
        };
        let mut changed_nodes = Vec::new();
        {
            let Some(node) = self.node_mut(&path) else {
                return false;
            };
            if node.children.is_empty() {
                return false;
            }
            if node.collapsed != collapse {
                node.collapsed = collapse;
                changed_nodes.push(node.id.clone());
            }
            if recursive {
                node.set_descendants_collapsed(collapse, &mut changed_nodes);
            }
        }
        if changed_nodes.is_empty() {
            return false;
        }
        self.pending_collapsed.extend(changed_nodes);
        self.detail_scroll = 0;
        self.rebuild_visible_paths();
        true
    }

    fn rebuild_visible_paths(&mut self) {
        self.visible_paths.clear();
        let mut current = Vec::new();
        Self::collect_paths(&self.root, &mut current, &mut self.visible_paths);
        if self.visible_paths.is_empty() {
            self.selected = 0;
            self.list_scroll = 0;
            return;
        }
        if self.selected >= self.visible_paths.len() {
            self.selected = self.visible_paths.len().saturating_sub(1);
        }
        self.list_scroll = self
            .list_scroll
            .min(self.visible_paths.len().saturating_sub(1));
    }

    fn collect_paths(
        node: &OverlayNode,
        prefix: &mut Vec<usize>,
        visible_paths: &mut Vec<Vec<usize>>,
    ) {
        for (idx, child) in node.children.iter().enumerate() {
            prefix.push(idx);
            visible_paths.push(prefix.clone());
            if !child.collapsed {
                Self::collect_paths(child, prefix, visible_paths);
            }
            prefix.pop();
        }
    }

    fn node(&self, path: &[usize]) -> Option<&OverlayNode> {
        let mut current = &self.root;
        for &idx in path {
            current = current.children.get(idx)?;
        }
        Some(current)
    }

    fn node_mut(&mut self, path: &[usize]) -> Option<&mut OverlayNode> {
        let mut current = &mut self.root;
        for &idx in path {
            current = current.children.get_mut(idx)?;
        }
        Some(current)
    }

    fn save_and_exit(&mut self) -> bool {
        self.is_done = true;
        self.exit_action = Some(ExitAction::Save);
        true
    }

    fn cancel_exit(&mut self) -> bool {
        self.is_done = true;
        self.exit_action = Some(ExitAction::Cancel);
        true
    }
}

impl OverlayNode {
    fn set_descendants_collapsed(&mut self, collapsed: bool, changed: &mut Vec<String>) {
        for child in &mut self.children {
            if child.collapsed != collapsed {
                child.collapsed = collapsed;
                changed.push(child.id.clone());
            }
            child.set_descendants_collapsed(collapsed, changed);
        }
    }

    fn set_descendants_selected(&mut self, selected: bool, changed: &mut Vec<String>) {
        if self.selected != selected {
            self.selected = selected;
            changed.push(self.id.clone());
        }
        for child in &mut self.children {
            child.set_descendants_selected(selected, changed);
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ExitAction {
    Save,
    Cancel,
}

fn render_hint_segment(keys: &[KeyBinding], desc: &str) -> Vec<Span<'static>> {
    if keys.is_empty() {
        return Vec::new();
    }
    let mut spans = vec![Span::from("  ")];
    for (i, key) in keys.iter().enumerate() {
        if i > 0 {
            spans.push(Span::from("/"));
        }
        spans.push(Span::from(*key));
    }
    spans.push(Span::from(format!(" {desc}")));
    spans
}

fn detail_lines_for_node(node: &OverlayNode) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.push(Line::from(node.title.clone().bold()));
    if !node.labels.is_empty() {
        lines.push(Line::from(vec![
            "Labels: ".dim(),
            node.labels.join("/").into(),
        ]));
    }
    if node.summary.is_empty() {
        lines.push(Line::from("No summary available.".dim()));
    } else {
        for part in node.summary.lines() {
            lines.push(render_markdown_line(part));
        }
    }
    lines.push(Line::from(String::new()));
    lines.push(Line::from(vec!["Node ID: ".dim(), node.id.clone().into()]));
    lines
}

fn render_markdown_line(line: &str) -> Line<'static> {
    let trimmed_start = line.trim_start();
    let indent_len = line.len().saturating_sub(trimmed_start.len());
    let indent = &line[..indent_len];
    let content = trimmed_start.trim();

    let mut spans = Vec::new();
    if !indent.is_empty() {
        spans.push(Span::from(indent.to_string()));
    }

    if content.is_empty() {
        spans.push(Span::from(String::new()));
        return Line::from(spans);
    }

    let mut base_style = Style::default();
    let heading_prefixes = ["###### ", "##### ", "#### ", "### ", "## ", "# "];
    for prefix in heading_prefixes {
        if let Some(stripped) = content.strip_prefix(prefix) {
            base_style = base_style.add_modifier(Modifier::BOLD);
            let remainder = stripped.trim_start();
            let mut inline = inline_spans(remainder, base_style);
            spans.append(&mut inline);
            return Line::from(spans);
        }
    }

    let mut remainder = content;
    if remainder.starts_with("- ") || remainder.starts_with("* ") {
        spans.push("• ".dim());
        remainder = remainder[2..].trim_start();
    }

    let mut inline = inline_spans(remainder, base_style);
    spans.append(&mut inline);
    if spans.is_empty() {
        spans.push(Span::from(String::new()));
    }
    Line::from(spans)
}

fn inline_spans(text: &str, base_style: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut buffer = String::new();
    let mut bold = false;
    let mut italic = false;
    let chars: Vec<char> = text.chars().collect();
    let mut idx = 0;
    while idx < chars.len() {
        if chars[idx] == '*' {
            let double_star = idx + 1 < chars.len() && chars[idx + 1] == '*';
            flush_markdown_span(&mut spans, &mut buffer, base_style, bold, italic);
            if double_star {
                bold = !bold;
                idx += 2;
            } else {
                italic = !italic;
                idx += 1;
            }
            continue;
        }
        buffer.push(chars[idx]);
        idx += 1;
    }
    flush_markdown_span(&mut spans, &mut buffer, base_style, bold, italic);
    if spans.is_empty() {
        spans.push(Span::from(String::new()));
    }
    spans
}

fn flush_markdown_span(
    spans: &mut Vec<Span<'static>>,
    buffer: &mut String,
    base_style: Style,
    bold: bool,
    italic: bool,
) {
    if buffer.is_empty() {
        return;
    }
    let mut style = base_style;
    if bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    if italic {
        style = style.add_modifier(Modifier::ITALIC);
    }
    spans.push(Span::styled(buffer.clone(), style));
    buffer.clear();
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SelectionState {
    Selected,
    Unselected,
    Partial,
    None,
}

fn selection_state(node: &OverlayNode) -> SelectionState {
    if node.children.is_empty() {
        return if node.selected {
            SelectionState::Selected
        } else {
            SelectionState::Unselected
        };
    }
    let mut total = 0;
    let mut selected = 0;
    for child in &node.children {
        match selection_state(child) {
            SelectionState::Selected => {
                selected += 1;
                total += 1;
            }
            SelectionState::Unselected => {
                total += 1;
            }
            SelectionState::Partial => {
                return SelectionState::Partial;
            }
            SelectionState::None => {}
        }
    }
    if total == 0 {
        SelectionState::None
    } else if selected == total {
        SelectionState::Selected
    } else if selected == 0 {
        SelectionState::Unselected
    } else {
        SelectionState::Partial
    }
}

fn selection_marker(state: SelectionState) -> &'static str {
    match state {
        SelectionState::Selected => "[x]",
        SelectionState::Unselected => "[ ]",
        SelectionState::Partial => "[~]",
        SelectionState::None => "[ ]",
    }
}
