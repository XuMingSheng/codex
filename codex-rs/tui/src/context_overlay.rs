use crate::key_hint;
use crate::key_hint::KeyBinding;
use crate::text_formatting::truncate_text;
use crate::tui;
use crate::tui::TuiEvent;
use crate::wrapping::RtOptions;
use crate::wrapping::word_wrap_lines;
use codex_protocol::models::ContentItem;
use codex_protocol::models::LocalShellAction;
use codex_protocol::models::ResponseItem;
use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyEventKind;
use crossterm::event::KeyModifiers;
use ratatui::buffer::Buffer;
use ratatui::layout::Constraint;
use ratatui::layout::Direction;
use ratatui::layout::Layout;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::widgets::Clear;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use ratatui::widgets::Wrap;
use std::io::Result;

const KEY_UP: KeyBinding = key_hint::plain(KeyCode::Up);
const KEY_DOWN: KeyBinding = key_hint::plain(KeyCode::Down);
const KEY_J: KeyBinding = key_hint::plain(KeyCode::Char('j'));
const KEY_K: KeyBinding = key_hint::plain(KeyCode::Char('k'));
const KEY_SCROLL_UP: KeyBinding = key_hint::plain(KeyCode::Char('['));
const KEY_SCROLL_DOWN: KeyBinding = key_hint::plain(KeyCode::Char(']'));
const KEY_PAGE_UP: KeyBinding = key_hint::plain(KeyCode::PageUp);
const KEY_PAGE_DOWN: KeyBinding = key_hint::plain(KeyCode::PageDown);
const KEY_CLOSE_Q: KeyBinding = key_hint::plain(KeyCode::Char('q'));
const KEY_CLOSE_ESC: KeyBinding = key_hint::plain(KeyCode::Esc);

pub(crate) struct ContextOverlay {
    entries: Vec<ContextEntry>,
    selected: usize,
    list_scroll: usize,
    detail_scroll: usize,
    last_detail_area_height: u16,
    last_detail_total_height: usize,
    is_done: bool,
}

impl ContextOverlay {
    pub(crate) fn new(items: Vec<ResponseItem>) -> Self {
        let entries = if items.is_empty() {
            vec![ContextEntry::from_strings(
                "conversation is empty",
                vec!["Nothing has been sent to the model yet.".to_string()],
            )]
        } else {
            items
                .into_iter()
                .enumerate()
                .map(|(idx, item)| ContextEntry::from_response(idx, item))
                .collect()
        };
        Self {
            entries,
            selected: 0,
            list_scroll: 0,
            detail_scroll: 0,
            last_detail_area_height: 0,
            last_detail_total_height: 0,
            is_done: false,
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

    fn handle_key(&mut self, key_event: KeyEvent) -> bool {
        if key_event.kind != KeyEventKind::Press && key_event.kind != KeyEventKind::Repeat
        {
            return false;
        }
        match key_event.code {
            KeyCode::Char('q') | KeyCode::Esc => {
                self.is_done = true;
                true
            }
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::PageUp => self.scroll_detail(-(self.page_scroll_amount() as isize)),
            KeyCode::PageDown => self.scroll_detail(self.page_scroll_amount() as isize),
            KeyCode::Char('[') => self.scroll_detail(-1),
            KeyCode::Char(']') => self.scroll_detail(1),
            KeyCode::Home if key_event.modifiers.contains(KeyModifiers::CONTROL) => {
                self.detail_scroll = 0;
                true
            }
            KeyCode::End if key_event.modifiers.contains(KeyModifiers::CONTROL) => {
                self.detail_scroll = usize::MAX;
                true
            }
            _ => {
                // Provide WASD-style aliases for selection when arrow keys unavailable.
                match key_event.code {
                    KeyCode::Char('w') => self.move_selection(-1),
                    KeyCode::Char('s') => self.move_selection(1),
                    _ => false,
                }
            }
        }
    }

    fn page_scroll_amount(&self) -> usize {
        self.last_detail_area_height.max(1) as usize
    }

    fn move_selection(&mut self, delta: isize) -> bool {
        if self.entries.is_empty() {
            return false;
        }
        let len = self.entries.len() as isize;
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
        if delta == 0 {
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
        self.render_detail(chunks[1], buf);
    }

    fn render_list(&mut self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let visible = area.height as usize;
        self.ensure_selection_visible(visible);
        let mut lines: Vec<Line<'static>> = Vec::with_capacity(visible);
        for (idx, entry) in self
            .entries
            .iter()
            .enumerate()
            .skip(self.list_scroll)
            .take(visible)
        {
            let label = format!("{:>2} {}", idx + 1, entry.label);
            if idx == self.selected {
                lines.push(Line::from(label.cyan().bold()));
            } else {
                lines.push(Line::from(label));
            }
        }
        if lines.is_empty() {
            lines.push(Line::from("No entries".dim()));
        }
        Paragraph::new(lines).render(area, buf);
    }

    fn ensure_selection_visible(&mut self, visible: usize) {
        if visible == 0 {
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
        let entry = self
            .entries
            .get(self.selected)
            .unwrap_or_else(|| self.entries.first().expect("at least one entry"));
        let wrapped = word_wrap_lines(entry.lines.clone(), RtOptions::new(area.width as usize));
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
        spans.extend(render_hint_segment(&[KEY_UP, KEY_DOWN, KEY_J, KEY_K], "select"));
        spans.extend(render_hint_segment(
            &[KEY_SCROLL_UP, KEY_SCROLL_DOWN, KEY_PAGE_UP, KEY_PAGE_DOWN],
            "scroll detail",
        ));
        spans.extend(render_hint_segment(
            &[KEY_CLOSE_Q, KEY_CLOSE_ESC],
            "close",
        ));
        Paragraph::new(Line::from(spans)).render(area, buf);
    }
}

fn render_hint_segment(keys: &[KeyBinding], desc: &str) -> Vec<Span<'static>> {
    if keys.is_empty() {
        return Vec::new();
    }
    let mut spans: Vec<Span<'static>> = vec![Span::from("  ")];
    for (i, key) in keys.iter().enumerate() {
        if i > 0 {
            spans.push(Span::from("/"));
        }
        spans.push(Span::from(*key));
    }
    spans.push(Span::from(format!(" {desc}")));
    spans
}

struct ContextEntry {
    label: String,
    lines: Vec<Line<'static>>,
}

impl ContextEntry {
    fn from_response(idx: usize, item: ResponseItem) -> Self {
        let (label, detail) = describe_item(idx, &item);
        let lines = detail
            .lines()
            .map(|line| Line::from(line.to_string()))
            .collect();
        Self { label, lines }
    }

    fn from_strings(label: &str, lines: Vec<String>) -> Self {
        let lines = lines
            .into_iter()
            .map(|line| Line::from(line))
            .collect::<Vec<_>>();
        Self {
            label: label.to_string(),
            lines,
        }
    }
}

fn describe_item(idx: usize, item: &ResponseItem) -> (String, String) {
    match item {
        ResponseItem::Message { role, content, .. } => {
            let body = render_content_items(content);
            let snippet = truncate_text(&body, 40);
            (
                format!("{role}: {snippet}"),
                format!("Message #{idx}\nRole: {role}\n\n{body}"),
            )
        }
        ResponseItem::Reasoning { summary, content, .. } => {
            let mut parts: Vec<String> = summary
                .iter()
                .filter_map(|s| match s {
                    codex_protocol::models::ReasoningItemReasoningSummary::SummaryText { text } => {
                        Some(text.clone())
                    }
                })
                .collect();
            if let Some(content) = content {
                for c in content {
                    if let codex_protocol::models::ReasoningItemContent::ReasoningText { text } = c {
                        parts.push(text.clone());
                    }
                }
            }
            let body = if parts.is_empty() {
                "No reasoning text provided.".to_string()
            } else {
                parts.join("\n\n")
            };
            ("reasoning".to_string(), body)
        }
        ResponseItem::FunctionCall {
            name, arguments, ..
        } => {
            let args = pretty_json(arguments).unwrap_or_else(|| arguments.clone());
            (
                format!("function call: {name}"),
                format!("Function: {name}\nArguments:\n{args}"),
            )
        }
        ResponseItem::FunctionCallOutput { call_id, output } => {
            let success = output.success.unwrap_or(true);
            (
                format!("function output ({call_id})"),
                format!(
                    "Function output (success={}):\n{}",
                    success,
                    output.content
                ),
            )
        }
        ResponseItem::CustomToolCall { name, input, .. } => (
            format!("tool call: {name}"),
            format!("Tool call `{name}` with input:\n{input}"),
        ),
        ResponseItem::CustomToolCallOutput { call_id, output } => (
            format!("tool output ({call_id})"),
            format!("Tool output:\n{output}"),
        ),
        ResponseItem::LocalShellCall { status, action, .. } => {
            let details = match action {
                LocalShellAction::Exec(exec) => {
                    let cmd = exec.command.join(" ");
                    format!("Command: {cmd}\nStatus: {status:?}")
                }
            };
            ("local shell call".to_string(), details)
        }
        ResponseItem::WebSearchCall { action, .. } => (
            "web search".to_string(),
            format!("Search action: {action:?}"),
        ),
        ResponseItem::GhostSnapshot { .. } => (
            "ghost snapshot".to_string(),
            "Ghost snapshot (not sent to the model).".to_string(),
        ),
        ResponseItem::Other => ("other item".to_string(), format!("{item:?}")),
    }
}

fn render_content_items(items: &[ContentItem]) -> String {
    let mut out: Vec<String> = Vec::new();
    for item in items {
        match item {
            ContentItem::InputText { text } => out.push(text.clone()),
            ContentItem::OutputText { text } => out.push(text.clone()),
            ContentItem::InputImage { image_url } => {
                out.push(format!("[image] {image_url}"));
            }
        }
    }
    if out.is_empty() {
        "—".to_string()
    } else {
        out.join("\n")
    }
}

fn pretty_json(raw: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(raw)
        .map(|value| serde_json::to_string_pretty(&value).unwrap_or_else(|_| raw.to_string()))
        .ok()
}
