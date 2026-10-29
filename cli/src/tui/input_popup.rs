use crate::tui::textarea::{
    InputEditEffect, active_style, cursor_line_style, cursor_style, get_text,
};
use crossterm::event::{
    Event,
    KeyCode::{self},
    KeyEvent,
};
use ratatui::layout::HorizontalAlignment;
use ratatui::layout::Margin;
use ratatui_textarea::{CursorMove, Input, TextArea};
use std::fmt;

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::Stylize,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Widget},
};

type ValidatorFn = for<'a> fn(&'a str) -> Result<&'a str, Line<'static>>;

pub struct InputPopup<'c> {
    title: String,
    size: [Constraint; 2],
    validator: ValidatorFn,
    textarea: TextArea<'c>,
}
impl<'c> InputPopup<'c> {
    pub fn new(
        title: String,
        size: [Constraint; 2],
        initial_value: &str,
        validator: ValidatorFn,
    ) -> Self {
        let mut textarea = TextArea::from([initial_value]);
        textarea.move_cursor(CursorMove::End);

        textarea.set_placeholder_text("Enter input...");

        textarea.set_cursor_line_style(cursor_line_style());
        textarea.set_cursor_style(cursor_style());

        textarea.set_block(
            Block::new()
                .borders(Borders::TOP | Borders::RIGHT | Borders::BOTTOM)
                .border_style(active_style()),
        );

        Self {
            title,
            size,
            validator,
            textarea,
        }
    }

    pub fn handle_event(&mut self, event: Event) -> InputEditEffect<String, Line<'static>> {
        // Handle enter pressed
        match event {
            Event::Key(KeyEvent {
                code: KeyCode::Enter,
                ..
            }) => {
                let text = get_text(&self.textarea);
                match (self.validator)(text) {
                    Ok(val) => return InputEditEffect::Applied(val.to_string()),
                    Err(err) => return InputEditEffect::Error(err),
                }
            }
            Event::Key(KeyEvent {
                code: KeyCode::Esc, ..
            }) => {
                return InputEditEffect::Aborted;
            }
            _ => {}
        }

        // Handle key input
        let input: Input = event.clone().into();
        self.textarea.input(input);

        InputEditEffect::Editing
    }
}

impl<'a> Widget for &InputPopup<'a> {
    fn render(self, rect: Rect, buffer: &mut Buffer) {
        let side_padding = 2;

        let label_text = Span::from("Edit: ").white();

        let label_text_width = label_text.width() as u16;
        let label = Paragraph::new(label_text).right_aligned();

        // Inner block
        let window_rect = rect.centered(self.size[0], self.size[1]);
        let inner_rect = window_rect.inner(Margin::new(1, 1));

        // Render the window decorations
        Clear.render(window_rect, buffer);
        Block::bordered()
            .title(self.title.as_str())
            .title_alignment(HorizontalAlignment::Center)
            .render(window_rect, buffer);

        // Split the layout in half, left for the label, right for the input.
        let label_and_input_layout = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(vec![
                Constraint::Length(label_text_width + side_padding),
                Constraint::Fill(1),
            ])
            .split(inner_rect);
        let label_rect = label_and_input_layout[0];
        let input_rect = label_and_input_layout[1];

        let input_label_block = Block::new()
            .borders(Borders::TOP | Borders::LEFT | Borders::BOTTOM)
            .border_style(active_style());

        label.block(input_label_block).render(label_rect, buffer);

        self.textarea.render(input_rect, buffer);
    }
}

impl fmt::Debug for InputPopup<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InputPopup")
            .field("textarea", &self.textarea)
            .finish_non_exhaustive()
    }
}
