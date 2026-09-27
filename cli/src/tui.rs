use crossterm::event::{
    self, Event,
    KeyCode::{self, Char},
    KeyEvent, KeyEventKind, KeyModifiers,
};
use ratatui_textarea::{CursorMove, Input, TextArea};
use std::collections::VecDeque;

use ratatui::{
    DefaultTerminal, Frame,
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style, Stylize},
    symbols::border,
    text::{Line, Text},
    widgets::{Block, Borders, Padding, Paragraph, Widget},
};
use std::io::{self};
use std::sync::mpsc::{Receiver, SyncSender};
use std::{thread, time};

use drumkick::{
    Command, DrumkickOpts, MAX_BPM, MIN_BPM, Response, ResponseLevel, Track, start_drumkick,
};

const UI_POLL_DURATION: time::Duration = time::Duration::from_millis(100);
const DEBUG_MESSAGES_CAPACITY: usize = 20;
const CONTROL: KeyModifiers = KeyModifiers::CONTROL;

fn get_input_text<'a>(input: &'a TextArea) -> &'a str {
    input.lines().first().map(String::as_str).unwrap_or("")
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
enum BPMInputEditEffect {
    Editing,
    Applied(f64),
    Error(String),
}

#[derive(Debug)]
pub struct BPMInput<'certainly_something> {
    input: TextArea<'certainly_something>,
}
impl BPMInput<'_> {
    fn cursor_line_style() -> Style {
        Style::default().fg(Color::Yellow)
    }
    fn cursor_style() -> Style {
        Style::default().bg(Color::DarkGray)
    }
    fn hidden_cursor_style() -> Style {
        // Setting the same style as cursor line hides a cursor.
        Self::cursor_line_style()
    }

    fn new(initial_bpm: f64) -> Self {
        let mut textarea = TextArea::from([initial_bpm.to_string()]);
        textarea.move_cursor(CursorMove::End);

        textarea.set_placeholder_text("Enter numeric BPM (eg. `80` or `75.5`)");

        textarea.set_cursor_line_style(Self::cursor_line_style());
        textarea.set_cursor_style(Self::hidden_cursor_style());

        textarea.set_block(Block::new().padding(Padding::top(2)));

        Self { input: textarea }
    }

    fn set_content(&mut self, content: &str) {
        self.input.clear();
        self.input.insert_str(content);
    }

    fn handle_event(&mut self, event: Event) -> BPMInputEditEffect {
        // Handle enter pressed
        if matches!(
            event,
            Event::Key(KeyEvent {
                code: KeyCode::Enter,
                ..
            })
        ) {
            let text = get_input_text(&self.input);
            if let Ok(new_bpm) = text.parse::<f64>() {
                let new_bpm = new_bpm.clamp(MIN_BPM, MAX_BPM);
                self.set_content(&new_bpm.to_string());
                return BPMInputEditEffect::Applied(new_bpm);
            } else {
                return BPMInputEditEffect::Error(format!(
                    "BPM must be a decimal number (e.g. \"60\" or \"115.5\") - got {:?}",
                    text
                ));
            }
        }

        // Handle key input
        let input: Input = event.clone().into();
        self.input.input(input);

        let text = get_input_text(&self.input);
        let rewritten_text = retain_real_number_chars(text);
        self.set_content(&rewritten_text);

        BPMInputEditEffect::Editing
    }

    /// Change appearance based on focus.
    ///
    /// No effect on input handling.
    fn set_focused(&mut self, focused: bool) {
        match focused {
            true => self.input.set_cursor_style(Self::cursor_style()),
            false => self.input.set_cursor_style(Self::hidden_cursor_style()),
        }
    }
}

impl<'a> Widget for &BPMInput<'a> {
    fn render(self, rect: Rect, buffer: &mut Buffer) {
        self.input.render(rect, buffer);
    }
}

#[derive(Debug)]
pub struct App<'textarea_block_why_is_it_even_here> {
    cmd_sender: SyncSender<Command>,
    resp_receiver: Receiver<Response>,
    bpm: f64,
    playing: bool,
    exit: bool,
    error: Option<String>,
    user_message: Option<String>,
    debug_messages: VecDeque<String>,
    bpm_input: BPMInput<'textarea_block_why_is_it_even_here>,
    bpm_input_focused: bool,
}

impl App<'_> {
    pub fn new(opts: DrumkickOpts, track: Track) -> Result<Self, Box<dyn std::error::Error>> {
        let bpm = track.bpm;
        let (cmd_sender, resp_receiver) = start_drumkick(opts, track)?;
        Ok(Self {
            cmd_sender,
            resp_receiver,
            bpm,
            playing: false,
            exit: false,
            error: None,
            user_message: None,
            debug_messages: VecDeque::new(),
            bpm_input: BPMInput::new(bpm),
            bpm_input_focused: false,
        })
    }

    pub fn run(
        &mut self,
        terminal: &mut DefaultTerminal,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.cmd_sender.send(Command::Play)?;
        self.playing = true;

        while !self.exit {
            terminal.draw(|frame| self.draw(frame))?;

            let mut event_received = false;
            while !event_received {
                event_received = self.handle_events()?;
            }
        }
        Ok(())
    }

    fn draw(&self, frame: &mut Frame) {
        let area = frame
            .area()
            .centered(Constraint::Max(80), Constraint::Max(30));
        let outer_block = Block::new();
        let outer_layout = Layout::default()
            .margin(1)
            .constraints(vec![Constraint::Min(1)])
            .split(outer_block.inner(area));

        // Main block
        let title = Line::from(vec![
            " drumkick".bold(),
            " - ".into(),
            self.user_message.as_deref().unwrap_or("").gray(),
            " ".into(),
        ]);

        let instructions = Line::from(if self.bpm_input_focused {
            vec![" Quit ".into(), "<^c> ".blue().bold()]
        } else {
            vec![
                " Play/Pause: ".into(),
                "<SPACE>".blue().bold(),
                " Adjust BPM: ".into(),
                "<+/-/Up/Down>".blue().bold(),
                " Quit ".into(),
                "<Q/^c> ".blue().bold(),
            ]
        });

        let block = Block::bordered()
            .padding(1.into())
            .title(title.centered())
            .title_bottom(instructions.centered())
            .border_set(border::THICK);

        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints(vec![
                Constraint::Max(4),
                // setting BPM
                Constraint::Fill(1),
                // Debug output
                Constraint::Fill(1),
            ])
            .split(block.inner(area));

        frame.render_widget(outer_block, area);
        frame.render_widget(block, outer_layout[0]);

        let bpm_setting_layout = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(vec![
                Constraint::Fill(1),
                Constraint::Max(1),
                Constraint::Fill(1),
            ])
            .split(layout[0]);

        // BPM input label
        frame.render_widget(
            Paragraph::new(Text::from(if self.bpm_input_focused {
                "New BPM:"
            } else {
                "BPM:"
            }))
            .right_aligned()
            .block(Block::new().padding(Padding::top(2))),
            bpm_setting_layout[0],
        );

        // BPM input
        frame.render_widget(&self.bpm_input, bpm_setting_layout[2]);

        // Status UI
        let bpm_action = if self.bpm_input_focused {
            "apply"
        } else {
            "input BPM"
        };
        let status_text = Text::from(vec![
            Line::from(vec!["<ENTER>".bold(), " - ".into(), bpm_action.into()]),
            Line::from(vec![]),
            Line::from(vec![self.error.as_deref().unwrap_or("").red()]),
        ]);
        frame.render_widget(
            Paragraph::new(status_text).centered().block(Block::new()),
            layout[1],
        );

        // Debug UI
        let debug_rect = layout[2];
        let lines_available = usize::from(debug_rect.height).saturating_sub(2);
        let debug_message_text = Text::from_iter(
            self.debug_messages
                .iter()
                .skip(self.debug_messages.len().saturating_sub(lines_available))
                .map(|m| Line::from(m.clone().dark_gray())),
        );
        frame.render_widget(
            Paragraph::new(debug_message_text).centered().block(
                Block::new()
                    .title(Line::from("Debug").centered())
                    .borders(Borders::TOP)
                    .border_set(border::PLAIN),
            ),
            debug_rect,
        );
    }

    fn handle_events(&mut self) -> io::Result<bool> {
        let mut backend_resp_available = false;
        for resp in self.resp_receiver.try_iter() {
            backend_resp_available = true;
            match resp {
                Response {
                    level: ResponseLevel::Debug,
                    message,
                } => {
                    self.debug_messages.push_back(message);
                    while self.debug_messages.len() > DEBUG_MESSAGES_CAPACITY {
                        self.debug_messages.pop_front();
                    }
                }
                Response {
                    level: ResponseLevel::User,
                    message,
                } => {
                    self.user_message = Some(message);
                }
            }
        }

        let terminal_event_available = event::poll(if backend_resp_available {
            time::Duration::from_secs(0)
        } else {
            UI_POLL_DURATION
        })?;
        let input_registered = backend_resp_available | terminal_event_available;

        if terminal_event_available {
            let event = event::read()?;

            // crossterm also emits key release and repeat events on Windows - skip other key
            // events
            if !matches!(
                event,
                Event::Key(KeyEvent {
                    kind: KeyEventKind::Press,
                    ..
                })
            ) {
                return Ok(input_registered);
            }
            // Now we know key inputs are meaningful

            if let Event::Key(key_event) = event {
                self.handle_global_key_event(key_event)
            }

            // Apply textarea input on enter
            if self.bpm_input_focused {
                self.handle_bpm_input_key_event(event);

                // Handle base-level UI events
            } else if let Event::Key(key_event) = event {
                self.handle_normal_key_event(key_event)
            }
        }

        Ok(input_registered)
    }

    fn handle_global_key_event(&mut self, key_event: KeyEvent) {
        if (key_event.code, key_event.modifiers) == (Char('c'), CONTROL) {
            self.exit();
        }
    }

    fn handle_normal_key_event(&mut self, key_event: KeyEvent) {
        match (key_event.code, key_event.modifiers) {
            (Char('q'), _) => self.exit(),
            (Char(' '), _) => self.toggle_pause(),
            (Char('+') | Char('=') | KeyCode::Up, _) => self.change_bpm(5.0),
            (Char('-') | Char('_') | KeyCode::Down, _) => self.change_bpm(-5.0),
            (KeyCode::Enter, _) => self.set_bpm_input_focused(true),
            _ => {}
        };
    }

    fn handle_bpm_input_key_event(&mut self, event: Event) {
        let effect = self.bpm_input.handle_event(event);
        match effect {
            BPMInputEditEffect::Editing => {}
            BPMInputEditEffect::Applied(bpm) => {
                self.set_bpm(bpm);
                self.set_bpm_input_focused(false)
            }
            BPMInputEditEffect::Error(err) => {
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
        if let Err(err) = self.cmd_sender.send(Command::Stop) {
            self.set_error(err);
        }

        thread::sleep(time::Duration::from_millis(50));
        self.exit = true;
    }

    fn toggle_pause(&mut self) {
        let command = if self.playing {
            Command::Pause
        } else {
            Command::Play
        };

        if let Err(err) = self.cmd_sender.send(command) {
            self.set_error(err);
        }

        self.playing = !self.playing;
    }

    fn set_bpm_input_focused(&mut self, focused: bool) {
        self.bpm_input_focused = focused;
        self.bpm_input.set_focused(focused);
    }

    fn change_bpm(&mut self, diff: f64) {
        self.set_bpm(self.bpm + diff)
    }

    fn set_bpm(&mut self, new_bpm: f64) {
        self.bpm = new_bpm;
        if let Err(err) = self.cmd_sender.send(Command::SetBPM(new_bpm)) {
            self.set_error(err);
        } else {
            self.clear_error();
        }
    }
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
