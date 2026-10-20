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
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style, Stylize},
    symbols::border,
    text::{Line, Span, Text},
    widgets::{Block, Borders, Padding, Paragraph, Widget},
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

        let label_text = Text::from(if self.focused { "New BPM: " } else { "BPM: " });
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
}
/// A copy-paste of BPMInput but don't worry - they will diverge as features are added.
impl MeasureInput<'_> {
    fn new(initial_text: &str) -> Self {
        let mut textarea = TextArea::from([initial_text]);
        textarea.move_cursor(CursorMove::End);

        textarea.set_placeholder_text("1:CLICK 2:click ...");

        textarea.set_cursor_line_style(cursor_line_style());
        textarea.set_cursor_style(hidden_cursor_style());

        textarea.set_block(Block::new().padding(Padding::uniform(1)));
        textarea.set_wrap_mode(WrapMode::Word);

        let mut input = Self { input: textarea };
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

    fn set_selected(&mut self, selected: bool) {
        match selected {
            true => self.input.set_block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(active_style()),
            ),
            false => self.input.set_block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().dark_gray()),
            ),
        }
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
struct Measure<'s> {
    id: u64,
    source: String,
    input: MeasureInput<'s>,
    sequence: Option<Sequence>,
}

#[derive(Debug)]
enum Dir {
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
enum AppFocus {
    #[default]
    App,
    BPMInput,
    Measure(usize),
}

#[derive(Debug)]
pub struct App<'certainly_something, 's> {
    drumkick: MachineHandle,
    exit: bool,
    error: Option<String>,
    debug_messages: VecDeque<String>,
    bpm_input: BPMInput<'certainly_something>,
    /// Menu navigation
    selection: AppFocus,
    /// Input focus
    focus: Vec<AppFocus>,
    next_measure_id: u64,
    measures: Vec<Measure<'s>>,
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
            error: None,
            debug_messages: VecDeque::new(),
            bpm_input: input,
            focus: vec![AppFocus::default()],
            selection: AppFocus::App,
            next_measure_id: 0,
            measures: Vec::new(),
            samples: track.samples,
        };
        app.add_measure(track.measure_beats.to_source(), Some(track.sequence));
        app.selection = AppFocus::App;
        Ok(app)
    }

    pub fn run(
        &mut self,
        terminal: &mut DefaultTerminal,
    ) -> Result<(), Box<dyn std::error::Error>> {
        while !self.exit {
            self.propagate();
            terminal.draw(|frame| self.draw(frame))?;

            let mut event_received = false;
            while !event_received {
                event_received = self.handle_events()?;
            }
        }
        Ok(())
    }

    fn focus(&self) -> AppFocus {
        self.focus.last().map_or_default(|f| f.to_owned())
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
        }

        // Mark selected measure (unless editing BPM)
        match self.selection {
            AppFocus::App => {}
            AppFocus::BPMInput => {
                self.bpm_input.set_selected(true);
            }
            AppFocus::Measure(measure_index) => {
                self.measures
                    .get_mut(measure_index)
                    .expect("selected measure")
                    .input
                    .set_selected(true);
            }
        }

        // Mark focused input (will render a cursor)
        match self.focus() {
            AppFocus::App => {}
            AppFocus::BPMInput => {
                self.bpm_input.set_focused(true);
            }
            AppFocus::Measure(n) => {
                let measure = self.measures.get_mut(n).expect("focused measure");
                measure.input.set_focused(true);
            }
        }
    }

    fn general_instructions(&self) -> Line<'_> {
        let mut instructions: Vec<Span> = vec![" ".into()];

        if self.focus() == AppFocus::App {
            instructions.append(&mut vec![
                "Space".blue().bold(),
                ": Play/Pause  ".into(),
                "B/+/-".blue().bold(),
                ": Set BPM  ".into(),
                "A".blue().bold(),
                ": Add measure  ".into(),
            ]);
        }

        if self.focus() == AppFocus::App {
            instructions.push("Q/^c".blue().bold());
        } else {
            instructions.push("^c".blue().bold());
        }
        instructions.push(": Quit ".into());

        Line::from(instructions)
    }

    fn draw(&self, frame: &mut Frame) {
        let Rect { width, height, .. } = frame.area();
        let area = frame.area().centered(
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

        // Main block
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

        let block = Block::bordered()
            .padding(1.into())
            .title(title.centered())
            .title_bottom(self.general_instructions().centered())
            .border_set(border::THICK);

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

        let bpm_rect = layout[0];
        let measures_rect = layout[1];
        let info_rect = layout[2];
        let action_rect = layout[3];

        frame.render_widget(block, outer_layout[0]);

        frame.render_widget(&self.bpm_input, bpm_rect);
        self.draw_measures(frame, measures_rect);
        self.draw_info(frame, info_rect);
        self.draw_action_instructions(frame, action_rect);
    }

    fn draw_measures(&self, frame: &mut Frame, rect: Rect) {
        let block = Block::new()
            .padding(1.into())
            .borders(Borders::TOP)
            .title("───Measures");

        frame.render_widget(&block, rect);
        let inner_rect = block.inner(rect);

        let mut constraints = vec![Constraint::Max(3); self.measures.len()];
        constraints.push(Constraint::Fill(1));
        let measures_list_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(inner_rect);

        let playback_progress = self.drumkick.playback_progress();

        for (&measure_rect, measure) in measures_list_layout.iter().zip(self.measures.iter()) {
            let measure_layout = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Fill(1), Constraint::Max(7)])
                .split(measure_rect);

            frame.render_widget(&measure.input, measure_layout[0]);

            let measure_progress = playback_progress
                .get(&measure.id)
                .map(ToString::to_string)
                .unwrap_or_else(|| "…".to_string());
            let measure_progress =
                Paragraph::new(Text::from(measure_progress)).block(Block::new().padding(1.into()));
            frame.render_widget(measure_progress, measure_layout[1]);
        }
    }

    fn draw_info(&self, frame: &mut Frame, rect: Rect) {
        let lines_available = usize::from(rect.height).saturating_sub(2);

        let mut error_text = Text::from(self.error.as_deref().unwrap_or("")).red();
        for msg in self
            .debug_messages
            .iter()
            .skip(self.debug_messages.len().saturating_sub(lines_available))
        {
            error_text.push_line(Line::from(msg.clone().dark_gray()));
        }

        let debug_block = Block::new()
            .title("───Info")
            .borders(Borders::TOP)
            .border_set(border::PLAIN);

        let debug_layout = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(vec![
                Constraint::Fill(1),
                Constraint::Max(error_text.width() as u16),
                Constraint::Fill(1),
            ])
            .split(debug_block.inner(rect));

        frame.render_widget(debug_block, rect);
        frame.render_widget(
            Paragraph::new(error_text).block(Block::new()),
            debug_layout[1],
        );
    }

    fn draw_action_instructions(&self, frame: &mut Frame, rect: Rect) {
        let mut line_elements: Vec<Span> = vec![];

        if self.focus() == AppFocus::App {
            line_elements.append(&mut vec!["Up/Down".bold(), ": Navigate".into()]);
        }

        let action_str = match (self.selection, self.focus()) {
            (_, AppFocus::BPMInput) => "Apply BPM",
            (_, AppFocus::Measure(_)) => "Apply measure",
            (AppFocus::App, AppFocus::App) => "",
            (AppFocus::BPMInput, _) => "Edit BPM",
            (AppFocus::Measure(_), _) => "Edit measure",
        };

        if !action_str.is_empty() {
            line_elements.append(&mut vec![
                "  ".into(),
                "Enter".bold(),
                ": ".into(),
                action_str.into(),
            ]);
        }

        if self.focus() == AppFocus::App && matches!(self.selection, AppFocus::Measure(_)) {
            line_elements.append(&mut vec![
                "  ".into(),
                "X".bold(),
                ": Delete measure".into(),
            ]);
        }

        if self.focus() != AppFocus::App {
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

    fn handle_events(&mut self) -> io::Result<bool> {
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

        let maybe_terminal_event = term::read_event_with_timeout(if backend_resp_available {
            time::Duration::from_secs(0)
        } else {
            UI_POLL_DURATION
        })?;
        let terminal_event_available = maybe_terminal_event.is_some();

        if let Some(event) = maybe_terminal_event {
            if let Event::Key(key_event) = event {
                self.handle_global_key_event(key_event)
            }

            match self.focus() {
                AppFocus::App => self.handle_normal_event(event),
                AppFocus::BPMInput => self.handle_bpm_input_key_event(event),
                AppFocus::Measure(index) => self.handle_measure_input_key_event(index, event),
            }
        }

        Ok(backend_resp_available | terminal_event_available)
    }

    fn handle_global_key_event(&mut self, key_event: KeyEvent) {
        if (key_event.code, key_event.modifiers) == (Char('c'), KeyModifiers::CONTROL) {
            self.exit();
        }
    }

    fn handle_normal_event(&mut self, event: Event) {
        if let Event::Key(key_event) = event {
            match (key_event.code, key_event.modifiers) {
                (Char('q'), _) => self.exit(),
                (Char(' '), _) => self.toggle_playing(),
                (Char('+') | Char('='), _) => self.change_bpm(5.0),
                (Char('-') | Char('_'), _) => self.change_bpm(-5.0),
                (Char('b'), _) => {
                    self.selection = AppFocus::BPMInput;
                    self.focus.push(AppFocus::BPMInput);
                }

                (KeyCode::Up, _) => self.navigate_measure_selection(Dir::Up),
                (KeyCode::Down, _) => self.navigate_measure_selection(Dir::Down),
                (KeyCode::Enter, _) => match self.selection {
                    AppFocus::App => {}
                    AppFocus::BPMInput => {
                        self.focus.push(AppFocus::BPMInput);
                    }
                    AppFocus::Measure(measure_index) => {
                        self.focus.push(AppFocus::Measure(measure_index));
                    }
                },
                (Char('a'), _) => self.add_measure(String::from("1:"), None),
                (Char('x'), _) => {
                    if let AppFocus::Measure(measure_index) = self.selection {
                        self.delete_measure(measure_index);
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
                self.focus.pop();
            }
            InputEditEffect::Applied(bpm) => {
                self.set_bpm(bpm);
                self.focus.pop();
            }
            InputEditEffect::Error(err) => {
                self.set_error(err);
            }
        }
    }

    fn handle_measure_input_key_event(&mut self, index: usize, event: Event) {
        let sample_names: Vec<_> = self.samples.keys().map(|k| k.as_str()).collect();

        let measure = self.measures.get_mut(index).expect("old_measure");
        let effect = measure.input.handle_event(event, &sample_names);

        match effect {
            InputEditEffect::Editing => {}
            InputEditEffect::Aborted => {
                measure.input.set_content(&measure.source);
                self.focus.pop();
            }
            InputEditEffect::Applied((source, beat_specs)) => {
                match samples::load_samples_into_sequence(&beat_specs, &self.samples) {
                    Err(err) => self.set_error(err),
                    Ok(sequence) => {
                        measure.source = source;
                        measure.sequence = Some(sequence);
                        self.update_sequences();
                        self.clear_error();
                        self.focus.pop();
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

    fn navigate_measure_selection(&mut self, dir: Dir) {
        let shifted_selection = match self.selection {
            AppFocus::BPMInput => 0,
            AppFocus::App => 1,
            AppFocus::Measure(measure_index) => measure_index + 2,
        };

        let new_active_measure_shifted = match dir {
            Dir::Up => shifted_selection.saturating_sub(1),
            Dir::Down => shifted_selection.saturating_add(1),
        };

        let new_active_measure = match new_active_measure_shifted {
            0 => AppFocus::BPMInput,
            1 => AppFocus::App,
            n => {
                let max_bound = self.measures.len() - 1;
                let index = (n - 2).min(max_bound);
                AppFocus::Measure(index)
            }
        };

        self.selection = new_active_measure;
    }

    fn add_measure(&mut self, source: String, sequence: Option<Sequence>) {
        self.measures.push(Measure {
            id: self.next_measure_id,
            input: MeasureInput::new(&source),
            source,
            sequence,
        });
        self.selection = AppFocus::Measure(self.measures.len() - 1);
        self.next_measure_id += 1;
        self.update_sequences();
    }

    fn delete_measure(&mut self, index: usize) {
        self.measures.remove(index);
        let measure_count = self.measures.len();
        if measure_count > 0 {
            self.selection = AppFocus::Measure(index.min(self.measures.len() - 1));
        } else {
            self.selection = AppFocus::App;
        }
        self.update_sequences();
    }

    fn update_sequences(&mut self) {
        self.drumkick.set_sequences(
            self.measures
                .iter()
                .filter_map(|m| m.sequence.as_ref().map(|s| (m.id, s))),
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
