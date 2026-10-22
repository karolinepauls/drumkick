use crate::parsing;
use crate::term;
use crossterm::event::{
    Event,
    KeyCode::{self, Char},
    KeyEvent, KeyModifiers,
};
use indexmap::IndexMap;
use ratatui_textarea::{CursorMove, Input, TextArea, WrapMode};
use std::collections::VecDeque;
use std::fmt;
use std::sync::Arc;

use ratatui::{
    DefaultTerminal, Frame,
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect, Size},
    style::{Color, Style, Stylize},
    symbols::border,
    text::{Line, Span, Text},
    widgets::{Block, Borders, Padding, Paragraph, Scrollbar, ScrollbarState, Widget},
};
use std::io;
use std::time;

use drumkick::{
    DrumkickOpts, InitialState, LoadedSource, MachineHandle, Response, ResponseLevel, Sequence,
    measure_grammar::{BeatSpec, MeasureParseError, ToSource as _, parse_measure},
    samples,
};

const UI_POLL_DURATION: time::Duration = time::Duration::from_millis(50);
const DEBUG_MESSAGES_CAPACITY: usize = 20;
const MEASURES_BLOCK_PADDING: u16 = 1;
const MEASURE_PADDING: u16 = 1;
const MEASURE_HEIGHT: u16 = 3;

fn get_input_text<'a>(input: &'a TextArea) -> &'a str {
    input.lines().first().map(String::as_str).unwrap_or("")
}

fn get_input_length(input: &TextArea) -> usize {
    input.lines().first().map(|s| s.len()).unwrap_or(0)
}

/// Retain decimal characters and only the first dot
fn retain_real_number_chars(input: &str) -> String {
    let mut real_number_chars_only = String::with_capacity(input.len());

    let mut dot_seen = false;
    for ch in input.chars() {
        if ch == '.' && !dot_seen {
            real_number_chars_only.push(ch);
            dot_seen = true;
        } else if ch.is_ascii_digit() {
            real_number_chars_only.push(ch);
        }
    }

    real_number_chars_only
}

#[derive(Debug)]
enum InputEditEffect<Ok, Err: fmt::Display> {
    Editing,
    Aborted,
    Applied(Ok),
    Error(Err),
}

fn active_style() -> Style {
    Style::default().light_green()
}

fn cursor_line_style() -> Style {
    Style::default().fg(Color::Yellow)
}
fn cursor_style() -> Style {
    Style::default().bg(Color::DarkGray)
}
fn hidden_cursor_style() -> Style {
    // Setting the same style as cursor line hides a cursor.
    cursor_line_style()
}

#[derive(Debug)]
pub struct BPMInput<'certainly_something> {
    input: TextArea<'certainly_something>,
    focused: bool,
    selected: bool,
}
impl BPMInput<'_> {
    fn new(initial_bpm: f64) -> Self {
        let mut textarea = TextArea::from([initial_bpm.to_string()]);
        textarea.move_cursor(CursorMove::End);

        textarea.set_placeholder_text("Enter numeric BPM (eg. `80` or `75.5`)");

        textarea.set_cursor_line_style(cursor_line_style());
        textarea.set_cursor_style(hidden_cursor_style());

        textarea.set_block(Block::new().padding(Padding::top(1)));

        Self {
            input: textarea,
            focused: false,
            selected: false,
        }
    }

    fn set_content(&mut self, content: &str) {
        self.input.clear();
        self.input.insert_str(content);
    }

    fn handle_event(&mut self, event: Event) -> InputEditEffect<f64, String> {
        // Handle enter pressed
        match event {
            Event::Key(KeyEvent {
                code: KeyCode::Enter,
                ..
            }) => {
                let text = get_input_text(&self.input);
                if let Ok(new_bpm) = parsing::parse_bpm(text) {
                    self.set_content(&new_bpm.to_string());
                    return InputEditEffect::Applied(new_bpm);
                } else {
                    return InputEditEffect::Error(format!(
                        "BPM must be a decimal number (e.g. \"60\" or \"115.5\") - got {:?}",
                        text
                    ));
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
        self.input.input(input);

        let text = get_input_text(&self.input);
        let rewritten_text = retain_real_number_chars(text);
        self.set_content(&rewritten_text);

        InputEditEffect::Editing
    }

    /// Change appearance based on focus.
    ///
    /// No effect on input handling.
    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        match focused {
            true => self.input.set_cursor_style(cursor_style()),
            false => self.input.set_cursor_style(hidden_cursor_style()),
        }
    }

    fn set_selected(&mut self, selected: bool) {
        self.selected = selected;
        let input_block = Block::default().borders(Borders::TOP | Borders::RIGHT | Borders::BOTTOM);

        self.input.set_block(match selected {
            true => input_block.border_style(active_style()),
            false => input_block.border_style(Style::default().black()),
        })
    }
}

impl<'a> Widget for &BPMInput<'a> {
    /// The rect needs to be exactly 3 lines high.
    fn render(self, rect: Rect, buffer: &mut Buffer) {
        let side_padding = 2;

        let label_text = Text::from(if self.focused {
            Span::from("New BPM: ").white()
        } else {
            Span::from("BPM: ")
        });
        let label_text_width = label_text.width() as u16;
        let bpm_label = Paragraph::new(label_text).right_aligned();

        // Split the layout in half, left for the label, right for the input.
        let bpm_setting_layout = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(vec![Constraint::Fill(1), Constraint::Fill(1)])
            .split(rect);
        let label_rect_outer = bpm_setting_layout[0];
        let input_rect_outer = bpm_setting_layout[1];

        // BPM input label
        let label_layout = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(vec![
                Constraint::Fill(1),
                Constraint::Length(label_text_width + side_padding),
            ])
            .split(label_rect_outer);
        let label_rect = label_layout[1];

        let mut bpm_input_label_block = Block::new()
            .borders(Borders::TOP | Borders::LEFT | Borders::BOTTOM)
            .border_style(Style::default().black());

        if self.selected {
            bpm_input_label_block = bpm_input_label_block.border_style(active_style());
        };
        bpm_label
            .block(bpm_input_label_block)
            .render(label_rect, buffer);

        // BPM input
        let input_length = get_input_length(&self.input) as u16;
        let input_layout = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(vec![
                Constraint::Length(input_length + side_padding),
                Constraint::Fill(1),
            ])
            .split(input_rect_outer);
        let input_rect = input_layout[0];

        self.input.render(input_rect, buffer);
    }
}

#[derive(Debug)]
pub struct MeasureInput<'certainly_something> {
    input: TextArea<'certainly_something>,
    block: Block<'certainly_something>,
}
/// A copy-paste of BPMInput but don't worry - they will diverge as features are added.
impl MeasureInput<'_> {
    fn new(initial_text: &str) -> Self {
        let mut textarea = TextArea::from([initial_text]);
        textarea.move_cursor(CursorMove::End);

        textarea.set_placeholder_text("1:CLICK 2:click ...");

        textarea.set_cursor_line_style(cursor_line_style());
        textarea.set_cursor_style(hidden_cursor_style());

        textarea.set_block(Block::new().padding(Padding::uniform(MEASURE_PADDING)));
        textarea.set_wrap_mode(WrapMode::Word);

        let mut input = Self {
            input: textarea,
            block: Block::default().borders(Borders::ALL),
        };
        input.set_selected(false);
        input
    }

    fn set_content(&mut self, content: &str) {
        self.input.clear();
        self.input.insert_str(content);
    }

    fn handle_event(
        &mut self,
        event: Event,
        all_sample_names: &[&str],
    ) -> InputEditEffect<(String, Vec<BeatSpec>), MeasureParseError> {
        // Handle enter pressed
        match event {
            Event::Key(KeyEvent {
                code: KeyCode::Enter,
                ..
            }) => {
                let text = get_input_text(&self.input);
                match parse_measure(text, all_sample_names) {
                    Ok(measure) => {
                        return InputEditEffect::Applied((text.to_string(), measure));
                    }
                    Err(err) => {
                        return InputEditEffect::Error(err);
                    }
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
        self.input.input(input);

        InputEditEffect::Editing
    }

    fn set_muted(&mut self, selected: bool) {
        match selected {
            true => {
                self.block = self.block.clone().border_set(border::LIGHT_DOUBLE_DASHED);
            }
            false => {
                self.block = self.block.clone().border_set(border::PLAIN);
            }
        }
        self.input.set_block(self.block.clone());
    }

    fn set_selected(&mut self, selected: bool) {
        match selected {
            true => {
                self.block = self.block.clone().border_style(active_style());
            }
            false => {
                self.block = self
                    .block
                    .clone()
                    .border_style(Style::default().dark_gray())
            }
        }
        self.input.set_block(self.block.clone());
    }

    /// Change appearance based on focus.
    ///
    /// No effect on input handling.
    fn set_focused(&mut self, focused: bool) {
        match focused {
            true => self.input.set_cursor_style(cursor_style()),
            false => self.input.set_cursor_style(hidden_cursor_style()),
        }
    }
}
impl<'a> Widget for &MeasureInput<'a> {
    fn render(self, rect: Rect, buffer: &mut Buffer) {
        self.input.render(rect, buffer);
    }
}

#[derive(Debug)]
struct Measure<'i> {
    id: u64,
    source: String,
    input: MeasureInput<'i>,
    sequence: Sequence,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
enum Selection {
    #[default]
    App,
    BPMInput {
        focus: bool,
    },
    Measure {
        index: usize,
        focus: bool,
    },
}

impl Selection {
    /// Check if there is any input focus (keys don't go to the main view)
    fn is_focused(&self) -> bool {
        match self {
            Self::App => false,
            Self::BPMInput { focus } | Self::Measure { focus, .. } => *focus,
        }
    }
}

#[derive(Debug, Clone)]
struct AppLayout {
    outer_rect: Rect,
    bpm_rect: Rect,
    measures_rect: Rect,
    info_rect: Rect,
    action_rect: Rect,
}

#[derive(Debug)]
pub struct App<'bpm, 'measure> {
    drumkick: MachineHandle,
    exit: bool,
    scroll_offset: u16,
    error: Option<String>,
    debug_messages: VecDeque<String>,
    bpm_input: BPMInput<'bpm>,
    /// Menu navigation and input focus
    selection: Vec<Selection>,
    next_measure_id: u64,
    measures: Vec<Measure<'measure>>,
    samples: IndexMap<String, Arc<LoadedSource>>,
}

impl App<'_, '_> {
    pub fn new(
        opts: DrumkickOpts,
        track: InitialState,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let drumkick = MachineHandle::new(opts, track.clone())?;
        let input = BPMInput::new(drumkick.bpm());
        let mut app = Self {
            drumkick,
            exit: false,
            scroll_offset: 0,
            error: None,
            debug_messages: VecDeque::new(),
            bpm_input: input,
            selection: vec![],
            next_measure_id: 0,
            measures: Vec::new(),
            samples: track.samples,
        };
        app.add_measure(
            app.measures.len(),
            track.measure_beats.to_source(),
            Some(track.sequence),
        );
        Ok(app)
    }

    pub fn run(
        &mut self,
        terminal: &mut DefaultTerminal,
    ) -> Result<(), Box<dyn std::error::Error>> {
        while !self.exit {
            self.propagate();

            let measures_block_height = Self::calculate_measures_block_height(terminal.size()?);
            self.adjust_scroll_offset_for_selection(measures_block_height);
            terminal.draw(|frame| self.draw(frame))?;

            let mut event_received = false;
            while !event_received {
                event_received = self.handle_events(measures_block_height)?;
            }
        }
        Ok(())
    }

    fn selection(&self) -> Selection {
        self.selection.last().map_or_default(|f| f.to_owned())
    }

    // Set selection, in-place.
    //
    // In order to manage the selection/focus stack, push to/pop from the vector.
    fn replace_selection(&mut self, new_focus: Selection) {
        if let Some(current) = self.selection.last_mut() {
            *current = new_focus;
        } else {
            self.selection.push(new_focus);
        }
    }

    /// Send data to child widgets.
    ///
    /// I don't think I like ratatui - it's too low level to be a React and at the same time
    /// pretends to be one by making the Widget.render method pure (not mut). StatefulWidget
    /// isn't - instead of the method being mut, it receives an additional mutable param. But does
    /// it mean that mutable sub-widgets (e.g. TextArea) need to be stored in that state?
    ///
    /// Anyway, this method resets all sub-widget state and then sets it according to focus.
    fn propagate(&mut self) {
        self.bpm_input.set_focused(false);
        self.bpm_input.set_selected(false);
        for measure in self.measures.iter_mut() {
            measure.input.set_focused(false);
            measure.input.set_selected(false);
            measure.input.set_muted(measure.sequence.muted);
        }

        // Mark focused input (will render a cursor)
        match self.selection() {
            Selection::App => {}
            Selection::BPMInput { focus } => {
                self.bpm_input.set_selected(true);
                if focus {
                    self.bpm_input.set_focused(true);
                }
            }
            Selection::Measure { index, focus } => {
                let measure = self.measures.get_mut(index).expect("focused measure");
                measure.input.set_selected(true);
                if focus {
                    measure.input.set_focused(true);
                }
            }
        }
    }

    fn calculate_measures_block_height(size: Size) -> u16 {
        let (AppLayout { measures_rect, .. }, _) =
            Self::app_level_layout(Rect::new(0, 0, size.width, size.height));

        measures_rect
            .height
            .saturating_sub(MEASURES_BLOCK_PADDING * 2)
            .saturating_sub(1) // correct for top border
    }

    fn calculate_measure_content_height(&self) -> u16 {
        (self.measures.len() as u16).saturating_mul(MEASURE_HEIGHT)
    }

    fn adjust_scroll_offset_for_selection(&mut self, measures_block_height: u16) {
        if let Selection::Measure { index, .. } = self.selection() {
            let selected_item_top = (index as u16).saturating_mul(MEASURE_HEIGHT);
            let selected_item_bottom = selected_item_top.saturating_add(MEASURE_HEIGHT);

            let display_top = self.scroll_offset;
            let display_bottom = self.scroll_offset.saturating_add(measures_block_height);

            if selected_item_bottom > display_bottom {
                self.scroll_offset = selected_item_bottom.saturating_sub(measures_block_height);
            } else if selected_item_top < display_top {
                self.scroll_offset = selected_item_top;
            }
        }
    }

    fn general_instructions(&self) -> Line<'_> {
        let mut instructions: Vec<Span> = vec![" ".into()];

        let focused = self.selection().is_focused();

        if !focused {
            instructions.append(&mut vec![
                "Space".blue().bold(),
                ": Play/Pause  ".into(),
                "B/+/-".blue().bold(),
                ": Set BPM  ".into(),
                "A".blue().bold(),
                ": Add measure  ".into(),
            ]);
        }

        if !focused {
            instructions.push("Q/^c".blue().bold());
        } else {
            instructions.push("^c".blue().bold());
        }
        instructions.push(": Quit ".into());

        Line::from(instructions)
    }

    fn app_level_layout<'a>(frame_rect: Rect) -> (AppLayout, Block<'a>) {
        let Rect { width, height, .. } = frame_rect;
        let area = frame_rect.centered(
            if width > 100 {
                Constraint::Percentage(80)
            } else {
                Constraint::Max(width - 2)
            },
            if height > 80 {
                Constraint::Percentage(70)
            } else {
                Constraint::Max(height - 2)
            },
        );
        let outer_block = Block::new();
        let outer_layout = Layout::default()
            .margin(1)
            .constraints(vec![Constraint::Min(1)])
            .split(outer_block.inner(area));

        // Main block, just dimensions
        let block = Block::bordered().padding(1.into());

        // Layout in the main block
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints(vec![
                // BPM layout and input
                Constraint::Max(3),
                // Measures/measures edit
                Constraint::Fill(2),
                // Info output
                Constraint::Fill(1),
                // Action
                Constraint::Max(1),
            ])
            .split(block.inner(area));

        (
            AppLayout {
                outer_rect: outer_layout[0],
                bpm_rect: layout[0],
                measures_rect: layout[1],
                info_rect: layout[2],
                action_rect: layout[3],
            },
            block,
        )
    }

    fn draw(&self, frame: &mut Frame) {
        let (
            AppLayout {
                outer_rect,
                bpm_rect,
                measures_rect,
                info_rect,
                action_rect,
            },
            block,
        ) = Self::app_level_layout(frame.area());

        // Draw main block
        let title = Line::from(vec![
            " drumkick".bold(),
            " - ".into(),
            (if self.drumkick.playing() {
                "playing"
            } else {
                "paused"
            })
            .into(),
            " ".into(),
        ]);
        let block = block
            .title(title.centered())
            .title_bottom(self.general_instructions().centered())
            .border_set(border::THICK);
        frame.render_widget(block, outer_rect);

        frame.render_widget(&self.bpm_input, bpm_rect);
        self.draw_measures(frame, measures_rect);
        self.draw_info(frame, info_rect);
        self.draw_action_instructions(frame, action_rect);
    }

    fn draw_measures(&self, frame: &mut Frame, rect: Rect) {
        let block = Block::new()
            .padding(MEASURES_BLOCK_PADDING.into())
            .borders(Borders::TOP)
            .title("───Measures");

        frame.render_widget(&block, rect);
        let inner_rect = block.inner(rect);

        let measures_and_scrollbar_layout =
            Layout::horizontal(vec![Constraint::Fill(1), Constraint::Length(1)]).split(inner_rect);
        let measures_rect = measures_and_scrollbar_layout[0];
        let scrollbar_rect = measures_and_scrollbar_layout[1];

        let content_length = self.calculate_measure_content_height();

        // Scrollbar
        let mut scrollbar_state = ScrollbarState::new(
            // Weird but needed to make the scrollbar act correctly. The content length passed into
            // the scrollbar has to be "the length that doesn't fit".
            content_length.saturating_sub(measures_rect.height).into(),
        )
        .position(self.scroll_offset.into())
        .viewport_content_length(measures_rect.height.into());
        frame.render_stateful_widget(Scrollbar::default(), scrollbar_rect, &mut scrollbar_state);

        // Scrolled measure list
        let scroll_buf_rect = Rect::new(0, 0, measures_rect.width, content_length);
        let mut scroll_buf = Buffer::empty(scroll_buf_rect);

        let measure_list_constraints = vec![Constraint::Max(3); self.measures.len()];
        let measures_list_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints(measure_list_constraints)
            .split(scroll_buf_rect);

        let playback_progress = self.drumkick.playback_progress();

        for (&measure_rect, measure) in measures_list_layout.iter().zip(self.measures.iter()) {
            let measure_layout = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Fill(1), Constraint::Max(7)])
                .split(measure_rect);

            measure.input.render(measure_layout[0], &mut scroll_buf);

            let measure_progress = playback_progress
                .get(&measure.id)
                .map(ToString::to_string)
                .unwrap_or_else(|| "…".to_string());
            let measure_progress = Paragraph::new(Text::from(measure_progress))
                .block(Block::new().padding(MEASURE_PADDING.into()));

            measure_progress.render(measure_layout[1], &mut scroll_buf);
        }

        // Write the visible section of the scroll buffer into the visible content.
        let visible_content = scroll_buf
            .content
            .into_iter()
            .skip((measures_rect.width.saturating_mul(self.scroll_offset)) as usize)
            .take(measures_rect.area() as usize);
        for (i, cell) in visible_content.enumerate() {
            let x = i as u16 % measures_rect.width;
            let y = i as u16 / measures_rect.width;
            frame.buffer_mut()[(measures_rect.x + x, measures_rect.y + y)] = cell;
        }
    }

    fn context_info(&self) -> Option<Line<'_>> {
        if let Selection::Measure { .. } = self.selection() {
            let mut line = Line::default();
            line.push_span(Span::from("Available samples: "));

            for sample_name in
                samples::all_available_sample_names(self.samples.keys().map(String::as_str))
            {
                line.push_span(Span::from(sample_name).light_magenta());
                line.push_span(Span::from(", ").white());
            }
            if line.spans.len() > 1 {
                line.spans.pop();
                line.push_span(Span::from(".").white());
            }

            Some(line)
        } else {
            None
        }
    }

    fn draw_info(&self, frame: &mut Frame, rect: Rect) {
        let lines_available = usize::from(rect.height).saturating_sub(2);

        let mut info_text = Text::default();

        if let Some(context_line) = self.context_info() {
            info_text.push_line(context_line.centered());
        }

        if let Some(ref error) = self.error {
            info_text.push_line(Line::default());
            for line in error.lines() {
                info_text.push_line(Line::from(line).light_red());
            }
        }

        for msg in self
            .debug_messages
            .iter()
            .skip(self.debug_messages.len().saturating_sub(lines_available))
        {
            info_text.push_line(Line::from(msg.clone().dark_gray()));
        }

        let debug_block = Block::new()
            .title("───Info")
            .borders(Borders::TOP)
            .border_set(border::PLAIN);

        let debug_layout = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(vec![
                Constraint::Fill(1),
                Constraint::Max(info_text.width() as u16),
                Constraint::Fill(1),
            ])
            .split(debug_block.inner(rect));

        frame.render_widget(debug_block, rect);
        frame.render_widget(
            Paragraph::new(info_text).block(Block::new()),
            debug_layout[1],
        );
    }

    fn draw_action_instructions(&self, frame: &mut Frame, rect: Rect) {
        let selection_focused = self.selection().is_focused();
        let mut line_elements: Vec<Span> = vec![];

        if !selection_focused {
            line_elements.append(&mut vec!["Up/Down".bold(), ": Navigate".into()]);
        }

        let action_str = match self.selection() {
            Selection::BPMInput { focus: false } => "Edit BPM",
            Selection::BPMInput { focus: true } => "Apply BPM",
            Selection::Measure { focus: false, .. } => "Edit", // No "measure" to preserve space
            Selection::Measure { focus: true, .. } => "Apply measure",
            Selection::App => "",
        };

        if !action_str.is_empty() {
            line_elements.append(&mut vec![
                "  ".into(),
                "Enter".bold(),
                ": ".into(),
                action_str.into(),
            ]);
        }

        if let Selection::Measure {
            index,
            focus: false,
        } = self.selection()
        {
            let measure = &self.measures[index];

            line_elements.append(&mut vec![
                "  ".into(),
                "M".bold(),
                ": ".into(),
                if measure.sequence.muted {
                    "Unmute"
                } else {
                    "Mute"
                }
                .into(),
            ]);

            line_elements.append(&mut vec!["  ".into(), "C".bold(), ": Clone".into()]);

            line_elements.append(&mut vec!["  ".into(), "X".bold(), ": Delete".into()]);
        }

        if selection_focused {
            line_elements.append(&mut vec!["  ".into(), "Esc".bold(), ": Cancel".into()]);
        }

        let status_text = Text::from(vec![Line::from(line_elements)]);
        frame.render_widget(
            Paragraph::new(status_text)
                .centered()
                .block(Block::new().on_blue()),
            rect,
        );
    }

    fn handle_events(&mut self, measures_block_height: u16) -> io::Result<bool> {
        let mut backend_resp_available = self.drumkick.playback_state_updated();
        for resp in self.drumkick.iter_resposes() {
            backend_resp_available = true;
            if let Response {
                level: ResponseLevel::Debug,
                message,
            } = resp
            {
                self.debug_messages.push_back(message);
                while self.debug_messages.len() > DEBUG_MESSAGES_CAPACITY {
                    self.debug_messages.pop_front();
                }
            }
        }

        // Process all terminal events until a timeout occurs
        let mut timeout = if backend_resp_available {
            time::Duration::from_secs(0)
        } else {
            UI_POLL_DURATION
        };
        let mut terminal_event_available = false;
        loop {
            let maybe_terminal_event = term::read_event_with_timeout(timeout)?;
            terminal_event_available |= maybe_terminal_event.is_some();
            timeout = time::Duration::from_secs(0);

            if let Some(event) = maybe_terminal_event {
                if let Event::Key(key_event) = event {
                    self.handle_global_key_event(key_event)
                }

                match self.selection() {
                    Selection::BPMInput { focus: true } => self.handle_bpm_input_key_event(event),
                    Selection::Measure {
                        focus: true, index, ..
                    } => self.handle_measure_input_key_event(index, event),
                    _ => self.handle_normal_event(event, measures_block_height),
                }
            } else {
                return Ok(backend_resp_available | terminal_event_available);
            }
        }
    }

    fn handle_global_key_event(&mut self, key_event: KeyEvent) {
        if (key_event.code, key_event.modifiers) == (Char('c'), KeyModifiers::CONTROL) {
            self.exit();
        }
    }

    fn handle_normal_event(&mut self, event: Event, measures_block_height: u16) {
        let measures_per_page = measures_block_height / MEASURE_HEIGHT;
        if let Event::Key(key_event) = event {
            match (key_event.code, key_event.modifiers) {
                (Char('q'), _) => self.exit(),
                (Char(' '), _) => self.toggle_playing(),
                (Char('+') | Char('='), _) => self.change_bpm(5.0),
                (Char('-') | Char('_'), _) => self.change_bpm(-5.0),
                (Char('b'), _) => {
                    self.selection.push(Selection::BPMInput { focus: true });
                }

                (KeyCode::Up, _) => self.navigate_selection(-1),
                (KeyCode::Down, _) => self.navigate_selection(1),
                (KeyCode::PageUp, _) => {
                    self.scroll_offset = self.scroll_offset.saturating_sub(measures_block_height);
                    self.navigate_measure_only_selection(-(measures_per_page as isize));
                }
                (KeyCode::PageDown, _) => {
                    self.scroll_offset = self
                        .scroll_offset
                        .saturating_add(measures_block_height)
                        .min(
                            self.calculate_measure_content_height()
                                .saturating_sub(measures_block_height),
                        );

                    self.navigate_measure_only_selection(measures_per_page as isize);
                }
                (KeyCode::Enter, _) => match self.selection() {
                    Selection::App => {}
                    Selection::BPMInput { .. } => {
                        self.selection.push(Selection::BPMInput { focus: true });
                    }
                    Selection::Measure { index, .. } => {
                        self.selection
                            .push(Selection::Measure { index, focus: true });
                    }
                },
                (Char('a'), _) => {
                    let index = self.add_measure(self.measures.len(), String::from("1:"), None);
                    self.selection.push(Selection::Measure {
                        index,
                        focus: false,
                    });
                }
                (Char('c'), _) => {
                    if let Selection::Measure { index, .. } = self.selection() {
                        self.clone_measure(index);
                    }
                }
                (Char('x'), _) => {
                    if let Selection::Measure { index, .. } = self.selection() {
                        self.delete_measure(index);
                    }
                }
                (Char('m'), _) => {
                    if let Selection::Measure { index, .. } = self.selection() {
                        self.mute_unmute_measure(index);
                    }
                }
                _ => {}
            };
        }
    }

    fn handle_bpm_input_key_event(&mut self, event: Event) {
        let effect = self.bpm_input.handle_event(event);
        match effect {
            InputEditEffect::Editing => {}
            InputEditEffect::Aborted => {
                self.bpm_input.set_content(&self.drumkick.bpm().to_string());
                self.selection.pop();
            }
            InputEditEffect::Applied(bpm) => {
                self.set_bpm(bpm);
                self.selection.pop();
            }
            InputEditEffect::Error(err) => {
                self.set_error(err);
            }
        }
    }

    fn handle_measure_input_key_event(&mut self, index: usize, event: Event) {
        let sample_names: Vec<_> =
            samples::all_available_sample_names(self.samples.keys().map(String::as_str)).collect();

        let measure = self.measures.get_mut(index).expect("old_measure");
        let effect = measure.input.handle_event(event, &sample_names);

        match effect {
            InputEditEffect::Editing => {}
            InputEditEffect::Aborted => {
                measure.input.set_content(&measure.source);
                self.selection.pop();
            }
            InputEditEffect::Applied((source, beat_specs)) => {
                match samples::arrange_samples_into_sequence_items(&beat_specs, &self.samples) {
                    Err(err) => self.set_error(err),
                    Ok(items) => {
                        measure.source = source;
                        measure.sequence.items = items;
                        self.update_sequences();
                        self.clear_error();
                        self.selection.pop();
                    }
                }
            }
            InputEditEffect::Error(err) => {
                self.set_error(err);
            }
        }
    }

    fn set_error(&mut self, error: impl ToString) {
        self.error = Some(error.to_string());
    }

    fn clear_error(&mut self) {
        self.error = None;
    }

    fn exit(&mut self) {
        if let Err(err) = self.drumkick.stop() {
            self.set_error(err);
        }
        self.exit = true;
    }

    fn toggle_playing(&mut self) {
        let result = self.drumkick.toggle_playing();

        if let Err(err) = result {
            self.set_error(err);
        } else {
            self.clear_error();
        };
    }

    fn change_bpm(&mut self, diff: f64) {
        self.set_bpm(self.drumkick.bpm() + diff)
    }

    fn set_bpm(&mut self, new_bpm: f64) {
        self.drumkick.set_bpm(new_bpm);
        self.bpm_input.set_content(&self.drumkick.bpm().to_string());
    }

    fn navigate_selection(&mut self, step: isize) {
        let non_measure_positions = 2;
        let shifted_selection = match self.selection() {
            Selection::BPMInput { .. } => 0,
            Selection::App => 1,
            Selection::Measure { index, .. } => index + non_measure_positions,
        };
        let new_active_measure_shifted = shifted_selection.saturating_add_signed(step);

        let new_active_measure = match new_active_measure_shifted {
            0 => Selection::BPMInput { focus: false },
            1 => Selection::App,
            _ if self.measures.is_empty() => Selection::App,
            n => {
                let max_bound = self.measures.len() - 1;
                let index = (n - non_measure_positions).min(max_bound);
                Selection::Measure {
                    index,
                    focus: false,
                }
            }
        };

        self.replace_selection(new_active_measure);
    }

    /// Navigate up/down within the measures block.
    fn navigate_measure_only_selection(&mut self, step: isize) {
        fn is_measure(selection: Selection) -> bool {
            matches!(selection, Selection::Measure { .. })
        }

        if self.measures.is_empty() {
            return;
        }

        // Page Down but measures are not selected - select measure 0 before
        if step > 0 && !is_measure(self.selection()) {
            self.replace_selection(Selection::Measure {
                index: 0,
                focus: false,
            });
        }

        let prev_selection = self.selection();
        self.navigate_selection(step);

        // Make sure that Page Up won't navigate past measure 0 into the app/bpm
        if step < 0 && is_measure(prev_selection) && !is_measure(self.selection()) {
            self.replace_selection(Selection::Measure {
                index: 0,
                focus: false,
            });
        }
    }

    fn add_measure(&mut self, index: usize, source: String, sequence: Option<Sequence>) -> usize {
        self.measures.insert(
            index,
            Measure {
                id: self.next_measure_id,
                input: MeasureInput::new(&source),
                source,
                sequence: sequence.unwrap_or_else(|| Sequence::new([])),
            },
        );
        self.replace_selection(Selection::Measure {
            index,
            focus: false,
        });
        self.next_measure_id += 1;
        self.update_sequences();

        index
    }

    fn clone_measure(&mut self, index: usize) -> usize {
        let existing_measure = &self.measures[index];
        self.add_measure(
            index + 1,
            existing_measure.source.clone(),
            Some(existing_measure.sequence.clone()),
        )
    }

    fn delete_measure(&mut self, index: usize) {
        self.measures.remove(index);
        let measure_count = self.measures.len();
        if measure_count > 0 {
            self.replace_selection(Selection::Measure {
                index: index.min(self.measures.len() - 1),
                focus: false,
            });
        } else {
            self.replace_selection(Selection::App);
        }
        self.update_sequences();
    }

    fn mute_unmute_measure(&mut self, index: usize) {
        let measure = &mut self.measures[index];
        measure.sequence.muted = !measure.sequence.muted;
        self.update_sequences();
    }

    fn update_sequences(&mut self) {
        self.drumkick.set_sequences(
            self.measures
                .iter()
                .filter(|m| !m.sequence.items.is_empty())
                .map(|m| (m.id, &m.sequence)),
        );
    }
}

pub fn run_tui(opts: DrumkickOpts, track: InitialState) -> Result<(), Box<dyn std::error::Error>> {
    let mut app = App::new(opts, track)?;
    ratatui::run(|terminal| app.run(terminal))
}

#[cfg(test)]
mod test {
    use super::*;
    use rstest::*;

    #[rstest]
    #[case("", "")]
    #[case("12345677890", "12345677890")]
    #[case("1234567789012345677890", "1234567789012345677890")]
    #[case("1231.1", "1231.1")]
    #[case(".9", ".9")]
    #[case("1231..1", "1231.1")]
    #[case("1231.1.", "1231.1")]
    #[case(".1", ".1")]
    #[case("..1", ".1")]
    #[case("asd", "")]
    #[case("asdąæπ²", "")]
    #[case("12asdąæπ²", "12")]
    #[case("1.2asdąæπ²", "1.2")]
    #[case("asdąæπ²12", "12")]
    #[case("asdąæπ²1.2", "1.2")]
    #[case("asd12ąæπ²", "12")]
    #[case("asd1.2ąæπ²", "1.2")]
    #[case(".", ".")]
    fn test_retain_real_number_chars(#[case] input: &str, #[case] retained: &str) {
        assert_eq!(retain_real_number_chars(input), retained,);
    }
}
