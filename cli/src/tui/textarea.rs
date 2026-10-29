use ratatui::style::{Color, Style};
use ratatui_textarea::TextArea;
use std::fmt;

#[derive(Debug)]
pub enum InputEditEffect<Ok, Err: fmt::Display> {
    Editing,
    Aborted,
    Applied(Ok),
    Error(Err),
}

pub fn active_style() -> Style {
    Style::default().light_green()
}

pub fn cursor_line_style() -> Style {
    Style::default().fg(Color::Yellow)
}
pub fn cursor_style() -> Style {
    Style::default().bg(Color::DarkGray)
}
pub fn hidden_cursor_style() -> Style {
    // Setting the same style as cursor line hides a cursor.
    cursor_line_style()
}

pub fn get_text<'a>(textarea: &'a TextArea) -> &'a str {
    textarea.lines().first().map(String::as_str).unwrap_or("")
}

pub fn get_length(textarea: &TextArea) -> usize {
    textarea.lines().first().map(|s| s.len()).unwrap_or(0)
}

pub fn set_text(textarea: &mut TextArea, content: &str) {
    textarea.clear();
    textarea.insert_str(content);
}

pub trait HasTextarea<'s> {
    fn textarea_mut(&mut self) -> &mut TextArea<'s>;

    fn set_textarea_text(&mut self, content: impl AsRef<str>) {
        set_text(self.textarea_mut(), content.as_ref());
    }
}
