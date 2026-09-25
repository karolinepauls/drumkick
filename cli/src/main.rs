use clap::{Parser, builder::RangedU64ValueParser, value_parser};
use crossterm::event::{
    self, Event,
    KeyCode::{self, Char},
    KeyEvent, KeyEventKind, KeyModifiers,
};
use indexmap::IndexMap;
use ratatui_textarea::{CursorMove, Input, TextArea};
use std::collections::VecDeque;

use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Direction, Layout},
    style::{Color, Style, Stylize},
    symbols::border,
    text::{Line, Text},
    widgets::{Block, Borders, Padding, Paragraph},
};
use std::collections::HashSet;
use std::fmt;
use std::io::{self};
use std::sync::mpsc::{Receiver, SyncSender};
use std::{thread, time};
use thiserror::Error;

use drumkick::{
    BUFFER_SIZE, Command, DrumkickOpts, MAX_BPM, MIN_BPM, PRECISION, Response, ResponseLevel,
    Track,
    embedded_samples::{EMBEDDED_SAMPLE_NAMES, get_embedded_sample_names},
    measure_grammar::{BEAT_FRACTIONS, BeatSpec, MeasureParseError, backtick_quote, parse_measure},
    pest_set_error_detail,
    samples::load_samples,
    start_drumkick,
};

const UI_POLL_DURATION: time::Duration = time::Duration::from_millis(100);
const DEBUG_MESSAGES_CAPACITY: usize = 20;
const CONTROL: KeyModifiers = KeyModifiers::CONTROL;

#[repr(i32)]
#[derive(Copy, Clone, Debug)]
enum ExitCodes {
    CantInterpretArgs = 10,
    CantLoadSamples = 11,
}

impl ExitCodes {
    fn code(&self) -> i32 {
        *self as i32
    }
}

#[derive(Parser, Debug)]
#[command(version, about, long_about = None, next_line_help = true)]
struct Args {
    #[arg(
        default_value_t = "1:CLICK 2:click, 3:click, 4:click".to_string(),
        long_help = format!(
            "Rhythm track specification in the form of <PLACEMENT>:<SAMPLE>[,<SAMPLE>...]

PLACEMENT can be a:
- beat number (`1`, `2`, ...)
- beat fraction name (e.g. `and`)
- or a literal fraction like `1/3`.

Supported beat fraction names are:
{}

The following SAMPLEs are always available:
{}
Other samples must be provided using --sample options.
",
            BEAT_FRACTIONS
                .iter()
                .map(|(name, value)| format!("- {name_q} → {value}", name_q = backtick_quote(name)))
                .collect::<Vec<_>>()
                .join("\n"),

            EMBEDDED_SAMPLE_NAMES
                .iter()
                .map(|name| format!("- {name_q}", name_q = backtick_quote(name)))
                .collect::<Vec<_>>()
                .join("\n"),
        )
    )]
    measure: String,

    #[arg(
        short = 's',
        long = "sample",
        help = "Sample name and location, e.g.: `-s kick=\"sample_path/kick.flac\"`
This will result in the `kick` sample being available in the `MEASURE` argument."
    )]
    samples: Vec<String>,

    #[arg(short, long, default_value_t = 60.0)]
    bpm: f64,

    #[arg(
        long,
        default_value_t = PRECISION,
        value_parser = RangedU64ValueParser::<usize>::new().range(1..=1024),
        help = "Beat scheduling precision in audio wave samples (lower → better precision, more CPU).
--precision=32 at 48kHz gives about 1.33ms jitter which should be acceptable in any application.
This argument shouldn't be necessary and all sounds should be scheduled at individual wave sample precision.
The high-level sound library used here (kira) can only schedule playback at the precision of its internal buffer size,
which can be configured by this argument."
    )]
    precision: usize,

    #[arg(
        long,
        default_value_t = BUFFER_SIZE,
        value_parser = value_parser!(u32).range(1..),
        help = "Audio host buffer size in audio wave samples (lower → better responsiveness, more CPU).
This setting may be ignored by the system audio server. Extreme values, unless ignored, may result in the app
being given insufficient time to produce the audio wave, resulting in buffer underruns and clicks/pops/silence."
    )]
    buffer_size: u32,
}

#[derive(Debug)]
struct TrackSpec<'a> {
    bpm: f64,
    measure_beats: Vec<BeatSpec>,
    sample_locations: IndexMap<&'a str, &'a str>,
}

#[derive(Error, Debug)]
#[error("samples must be defined like: `sample_name=sample/path.flac` - got: {}", .errors.join(", "))]
struct SampleLocationsError {
    errors: Vec<String>,
}

#[derive(Error, Debug)]
struct ArgsParseError {
    sample_locations: Option<SampleLocationsError>,
    measure_parse_errors: Option<MeasureParseError>,
}
impl fmt::Display for ArgsParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        if let Some(sample_locations) = &self.sample_locations {
            writeln!(f, "sample error - {}", sample_locations)?;
        }
        if let Some(measure_parse_errors) = &self.measure_parse_errors {
            writeln!(f, "measure syntax error\n{}", measure_parse_errors)?;
        }
        Ok(())
    }
}

fn unwrap_or_terminate<T, E: fmt::Display>(r: Result<T, E>, error_code: i32) -> T {
    match r {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(error_code);
        }
    }
}

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
pub struct App<'textarea_block_why_is_it_even_here> {
    cmd_sender: SyncSender<Command>,
    resp_receiver: Receiver<Response>,
    bpm: f64,
    playing: bool,
    exit: bool,
    error: Option<String>,
    user_message: Option<String>,
    debug_messages: VecDeque<String>,
    bpm_input: Option<TextArea<'textarea_block_why_is_it_even_here>>,
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
            bpm_input: None,
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

        let instructions = Line::from(if self.bpm_input.is_some() {
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

        let bpm_text = Text::from(vec![Line::from(vec![self.bpm.to_string().yellow()])]);

        let debug_message_text: Text = Text::from_iter(
            self.debug_messages
                .iter()
                .map(|m| Line::from(m.clone().dark_gray())),
        );

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

        frame.render_widget(
            Paragraph::new(Text::from(if self.bpm_input.is_some() {
                "New BPM:"
            } else {
                "BPM:"
            }))
            .right_aligned()
            .block(Block::new().padding(Padding::top(2))),
            bpm_setting_layout[0],
        );

        if let Some(bpm_input) = self.bpm_input.as_ref() {
            // BPM input
            frame.render_widget(bpm_input, bpm_setting_layout[2]);
        } else {
            let bpm_paragraph =
                Paragraph::new(bpm_text).block(Block::new().padding(Padding::top(2)).clone());

            frame.render_widget(bpm_paragraph, bpm_setting_layout[2]);
        }

        // Status UI
        let bpm_action = if self.bpm_input.is_some() {
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
        frame.render_widget(
            Paragraph::new(debug_message_text).centered().block(
                Block::new()
                    .title(Line::from("Debug").centered())
                    .borders(Borders::TOP)
                    .border_set(border::PLAIN),
            ),
            layout[2],
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

            if let Event::Key(key_event) = event {
                self.handle_global_key_event(key_event)
            }

            // Apply textarea input on enter
            if let Some(bpm_input) = self.bpm_input.as_mut()
                && matches!(
                    event,
                    Event::Key(KeyEvent {
                        code: KeyCode::Enter,
                        ..
                    })
                )
            {
                let text = get_input_text(bpm_input);
                if let Ok(new_bpm) = text.parse::<f64>() {
                    self.set_bpm(new_bpm);
                } else {
                    let error = format!(
                        "BPM must be a decimal number (e.g. \"60\" or \"115.5\") - got {:?}",
                        text
                    );
                    self.set_error(error)
                }
                self.bpm_input = None;
            }
            // Send input to the textarea if focused
            else if let Some(bpm_input) = self.bpm_input.as_mut() {
                let input: Input = event.clone().into();
                bpm_input.input(input);

                let text = get_input_text(bpm_input);
                let rewritten_text = retain_real_number_chars(text);
                bpm_input.clear();
                bpm_input.insert_str(rewritten_text);
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
            (Char('+') | KeyCode::Up, _) => self.change_bpm(5.0),
            (Char('-') | Char('_') | KeyCode::Down, _) => self.change_bpm(-5.0),
            (KeyCode::Enter, _) => self.edit_bpm(),
            _ => {}
        };
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

    fn change_bpm(&mut self, diff: f64) {
        self.set_bpm(self.bpm + diff)
    }

    fn set_bpm(&mut self, new_bpm: f64) {
        let new_bpm = new_bpm.clamp(MIN_BPM, MAX_BPM);
        self.bpm = new_bpm;
        if let Err(err) = self.cmd_sender.send(Command::SetBPM(new_bpm)) {
            self.set_error(err);
        } else {
            self.clear_error();
        }
    }

    fn edit_bpm(&mut self) {
        let mut textarea = TextArea::from([self.bpm.to_string()]);
        textarea.move_cursor(CursorMove::End);

        textarea.set_cursor_line_style(Style::default());
        textarea.set_placeholder_text("Enter numeric BPM (eg. `80` or `75.5`)");
        textarea.set_style(Style::default().fg(Color::DarkGray));
        textarea.set_cursor_line_style(Style::default().fg(Color::Yellow));

        textarea.set_block(Block::new().padding(Padding::top(2)));

        self.bpm_input = Some(textarea);
    }
}

fn init() -> Result<(DrumkickOpts, Track), Box<dyn std::error::Error>> {
    let args = Args::parse();
    pest_set_error_detail(true);

    let track_spec =
        unwrap_or_terminate(interpret_args(&args), ExitCodes::CantInterpretArgs.code());

    let beats = unwrap_or_terminate(
        load_samples(&track_spec.measure_beats, &track_spec.sample_locations),
        ExitCodes::CantLoadSamples.code(),
    );

    Ok((
        DrumkickOpts {
            buffer_size: args.buffer_size,
            precision: args.precision,
            sound_capacity: 512,
            message_buffer_size: 32,
        },
        Track {
            bpm: track_spec.bpm,
            measure_beats: beats,
        },
    ))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (opts, track) = init()?;
    eprintln!("Measure has {} beats", track.beats_in_measure());

    let mut app = App::new(opts, track)?;
    ratatui::run(|terminal| app.run(terminal))?;
    Ok(())
}

/// Produce a map of sample names to sample paths
fn parse_sample_locations(
    samples: &[String],
) -> Result<IndexMap<&str, &str>, SampleLocationsError> {
    let split_opts = samples.iter().map(|s| s.split_once('='));
    let mut errors = Vec::new();
    let mut split = IndexMap::new();
    for (sample, split_opt) in samples.iter().zip(split_opts) {
        match split_opt {
            Some((sample, path)) => {
                split.insert(sample, path);
            }
            None => errors.push(sample.to_owned()),
        }
    }

    if !errors.is_empty() {
        return Err(SampleLocationsError { errors });
    }

    Ok(split)
}

fn interpret_args<'a>(args: &'a Args) -> Result<TrackSpec<'a>, Box<ArgsParseError>> {
    let sample_locations = parse_sample_locations(&args.samples);

    // Build a vec of all sample names, embedded and provided, for error reporting.
    let mut all_sample_names = get_embedded_sample_names();
    let mut seen_sample_names: HashSet<_> = all_sample_names.iter().copied().collect();

    // Sample locations may have failed to load, in which case we won't append them to all_sample_names.
    if let Ok(locations) = sample_locations.as_ref() {
        for key in locations.keys() {
            let new = seen_sample_names.insert(key);
            if new {
                all_sample_names.push(key);
            }
        }
    }

    let measure_beats = parse_measure(&args.measure, &all_sample_names);

    match (sample_locations, measure_beats) {
        (Ok(sample_locations), Ok(measure_beats)) => Ok(TrackSpec {
            bpm: args.bpm,
            measure_beats,
            sample_locations,
        }),
        (sample_locations, measure_beats) => Err(Box::new(ArgsParseError {
            sample_locations: sample_locations.err(),
            measure_parse_errors: measure_beats.err(),
        })),
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
