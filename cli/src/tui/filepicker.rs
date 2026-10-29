use crate::tui::{
    plural::naive_plural_suffix,
    textarea::{cursor_line_style, cursor_style, get_text, hidden_cursor_style, set_text},
};
use crossterm::event::{
    Event,
    KeyCode::{self},
    KeyEvent,
};
use ratatui::{
    crossterm,
    prelude::*,
    widgets::Padding,
    widgets::{Block, Widget, WidgetRef},
};
use std::path::PathBuf;
use std::{io, path::Path};

use ratatui_explorer::{File, FileExplorer, FileExplorerBuilder, Theme};
use ratatui_textarea::{Input, TextArea};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FilepickerFocus {
    Files,
    Filter,
}

#[derive(Debug)]
pub enum FilepickerEffect {
    Picking,
    Closed,
    Added(File),
    Removed(PathBuf),
    Error(String),
}

#[derive(Debug)]
pub struct Filepicker<'c> {
    explorer: FileExplorer,
    filter_textarea: TextArea<'c>,
    previous_filter: String,
    external_focus: bool,
    focus: FilepickerFocus,
}

impl Filepicker<'_> {
    pub fn new() -> io::Result<Self> {
        let file_explorer = FileExplorerBuilder::default()
            // Allowing marking any files apart from "..".
            .theme(Theme::default())
            .mark_condition(|f| f.is_file())
            .build()?;

        let mut textarea = TextArea::default();
        textarea.set_placeholder_text("Press / to filter");
        textarea.set_cursor_line_style(cursor_line_style());
        textarea.set_cursor_style(hidden_cursor_style());

        textarea.set_block(Block::new().padding(Padding::left(1)));

        let mut filepicker = Self {
            filter_textarea: textarea,
            explorer: file_explorer,
            previous_filter: String::new(),
            external_focus: false,
            focus: FilepickerFocus::Files,
        };
        filepicker.update_styles();
        Ok(filepicker)
    }

    pub fn set_external_focus(&mut self, focus: bool) {
        self.external_focus = focus;
        self.update_styles();
    }

    pub fn mark_file(&mut self, file: File) {
        self.explorer.mark_file(file);
    }

    pub fn unmark_file(&mut self, path: &Path) {
        self.explorer.unmark_file(path);
    }

    pub fn handle_event(&mut self, event: Event) -> FilepickerEffect {
        let result = match self.focus {
            FilepickerFocus::Files => self.handle_normal_event(event),
            FilepickerFocus::Filter => self.handle_filter_event(event),
        };
        self.update_styles();

        result
    }

    pub fn current_file_marked(&self) -> bool {
        self.explorer.is_marked(&self.explorer.current().path)
    }

    fn handle_normal_event(&mut self, event: Event) -> FilepickerEffect {
        match event {
            Event::Key(KeyEvent {
                code: KeyCode::Esc | KeyCode::Char('q'),
                ..
            }) => FilepickerEffect::Closed,
            Event::Key(KeyEvent {
                code: KeyCode::Char(' '),
                ..
            }) => {
                let file = self.explorer.current().clone();
                if file.is_file() {
                    let path = file.path.clone();
                    if self.explorer.is_marked(&path) {
                        FilepickerEffect::Removed(path)
                    } else {
                        FilepickerEffect::Added(file)
                    }
                } else {
                    FilepickerEffect::Picking
                }
            }

            Event::Key(KeyEvent {
                code: KeyCode::Char('/'),
                ..
            }) => {
                self.focus = FilepickerFocus::Filter;
                FilepickerEffect::Picking
            }
            _ => {
                // Send input to the file explorer or filter input.
                if let io::Result::Err(err) = self.explorer.handle(&event) {
                    FilepickerEffect::Error(err.to_string())
                } else {
                    FilepickerEffect::Picking
                }
            }
        }
    }

    fn handle_filter_event(&mut self, event: Event) -> FilepickerEffect {
        match event {
            Event::Key(KeyEvent {
                code: KeyCode::Enter,
                ..
            }) => {
                self.previous_filter = get_text(&self.filter_textarea).to_string();
                self.focus = FilepickerFocus::Files;
            }
            Event::Key(KeyEvent {
                code: KeyCode::Esc, ..
            }) => {
                set_text(&mut self.filter_textarea, &self.previous_filter);
                self.focus = FilepickerFocus::Files;
            }
            _ => {
                let input: Input = event.into();
                self.filter_textarea.input(input);

                let filter_text = get_text(&self.filter_textarea).to_lowercase();
                let result = self.explorer.set_filter_map(move |file| {
                    // The widget crashes at FileExplorer.current if no files are present so always
                    // show "../"
                    if file.name == "../" || file.name.to_lowercase().contains(&filter_text) {
                        Some(file)
                    } else {
                        None
                    }
                });
                if let io::Result::Err(err) = result {
                    return FilepickerEffect::Error(err.to_string());
                }
            }
        }
        FilepickerEffect::Picking
    }

    fn update_styles(&mut self) {
        self.explorer.set_theme(get_theme(
            self.external_focus && self.focus == FilepickerFocus::Files,
            self.explorer.marked_count(),
        ));
        match self.focus {
            FilepickerFocus::Files => {
                self.filter_textarea.set_cursor_style(hidden_cursor_style());
            }
            FilepickerFocus::Filter => {
                self.filter_textarea.set_cursor_style(cursor_style());
            }
        }
    }
}

impl WidgetRef for Filepicker<'_> {
    fn render_ref(&self, rect: Rect, buffer: &mut Buffer) {
        let layout = Layout::vertical(vec![Constraint::Fill(1), Constraint::Length(1)]).split(rect);
        let explorer_rect = layout[0];
        let filter_rect = layout[1];

        self.filter_textarea.render(filter_rect, buffer);
        self.explorer.widget().render_ref(explorer_rect, buffer);
    }
}

fn get_theme(focused: bool, marked_files: usize) -> Theme {
    let theme = Theme::default()
        .with_title_bottom(move |_f| {
            Line::from_iter(vec![
                "───".into(),
                Span::raw(marked_files.to_string()).bold(),
                " marked file".into(),
                naive_plural_suffix(marked_files).into(),
                " ".into(),
                Span::raw("Space").bold(),
                ": mark/unmark ".into(),
            ])
        })
        .with_scroll_padding(1);
    if focused {
        theme
    } else {
        theme
            .with_highlight_item_style(Style::default())
            .with_highlight_dir_style(Style::default())
    }
}
