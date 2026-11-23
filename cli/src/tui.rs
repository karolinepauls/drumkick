mod block;
mod filepicker;
mod input_popup;
mod plural;
use crate::tui::plural::naive_plural_suffix;
use crate::tui::scrolly::NavCondition;
use crate::tui::scrolly::Scrolly;
use input_popup::InputPopup;
mod scrolly;
mod textarea;
use crate::parsing;
use crate::term;
use crate::tui::block::{DEFAULT_BLOCK_PADDING, subview_block};
use crate::tui::scrolly::ScrollyNav;
use crate::tui::scrolly::ScrollyParams;
use crate::tui::textarea::{
    HasTextarea, InputEditEffect, active_style, cursor_line_style, cursor_style, get_length,
    get_text, hidden_cursor_style, set_text,
};
use core::fmt::NumBuffer;
use crossterm::event::Event::Resize;
use crossterm::event::{
    Event,
    KeyCode::{self, Char},
    KeyEvent, KeyModifiers,
};
use indexmap::IndexMap;
use ratatui::style::Color;
use ratatui_explorer::File;
use ratatui_textarea::DataCursor;
use ratatui_textarea::{CursorMove, Input, TextArea, WrapMode};
use std::collections::VecDeque;
use std::path::Path;
use std::sync::Arc;

use ratatui::{
    DefaultTerminal, Frame,
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect, Size},
    style::{Style, Stylize},
    symbols::border,
    text::{Line, Span, Text},
    widgets::{Block, Borders, Padding, Paragraph, Widget, WidgetRef as _},
};
use std::io;
use std::time;

use crate::tui::filepicker::{Filepicker, FilepickerEffect};
use drumkick::{
    DrumkickOpts, InitialState, LoadedSource, MachineHandle, Response, ResponseLevel, Sequence,
    SourceOrigin, SymphoniaError, load_from_path,
    measure_grammar::{
        BeatSpec, MeasureParseError, ToSource as _, clean_sample_name, parse_measure,
        validate_sample_name,
    },
    samples,
};

const UI_POLL_DURATION: time::Duration = time::Duration::from_millis(50);
const DEBUG_MESSAGES_CAPACITY: usize = 20;
const MEASURE_PADDING: u16 = 1;
const MEASURE_HEIGHT: u16 = 3;

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

fn error_text(error: &str) -> Text<'static> {
    let mut error_text = Text::default();
    for line in error.lines() {
        // line.to_string() passes an owned String, making sure that the inner Cow is owned and its
        // lifetime can be 'static
        error_text.push_line(Line::from(line.to_string()).light_red());
    }
    error_text
}

#[derive(Debug)]
pub struct BPMInput<'certainly_something> {
    textarea: TextArea<'certainly_something>,
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
            textarea,
            focused: false,
            selected: false,
        }
    }

    fn handle_event(&mut self, event: Event) -> InputEditEffect<f64, String> {
        // Handle enter pressed
        match event {
            Event::Key(KeyEvent {
                code: KeyCode::Enter,
                ..
            }) => {
                let text = get_text(&self.textarea);
                if let Ok(new_bpm) = parsing::parse_bpm(text) {
                    set_text(&mut self.textarea, &new_bpm.to_string());
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
        self.textarea.input(input);

        let text = get_text(&self.textarea);
        let rewritten_text = retain_real_number_chars(text);
        set_text(&mut self.textarea, &rewritten_text);

        InputEditEffect::Editing
    }

    /// Change appearance based on focus.
    ///
    /// No effect on input handling.
    fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        match focused {
            true => self.textarea.set_cursor_style(cursor_style()),
            false => self.textarea.set_cursor_style(hidden_cursor_style()),
        }
    }

    fn set_selected(&mut self, selected: bool) {
        self.selected = selected;
        let input_block = Block::default().borders(Borders::TOP | Borders::RIGHT | Borders::BOTTOM);

        self.textarea.set_block(match selected {
            true => input_block.border_style(active_style()),
            false => input_block.border_style(Style::default().black()),
        })
    }
}

impl<'s> HasTextarea<'s> for BPMInput<'s> {
    fn textarea_mut(&mut self) -> &mut TextArea<'s> {
        &mut self.textarea
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
        let input_length = get_length(&self.textarea) as u16;
        let input_layout = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(vec![
                Constraint::Length(input_length + side_padding),
                Constraint::Fill(1),
            ])
            .split(input_rect_outer);
        let input_rect = input_layout[0];

        self.textarea.render(input_rect, buffer);
    }
}

#[derive(Debug)]
pub struct MeasureInput<'certainly_something> {
    textarea: TextArea<'certainly_something>,
    block: Block<'certainly_something>,
}
/// A copy-paste of BPMInput but don't worry - they will diverge as features are added.
impl MeasureInput<'_> {
    fn new(initial_text: &str, cursor_pos: CursorMove) -> Self {
        let mut textarea = TextArea::from([initial_text]);
        textarea.move_cursor(cursor_pos);

        textarea.set_placeholder_text("1:CLICK 2:click ...");

        textarea.set_cursor_line_style(cursor_line_style());
        textarea.set_cursor_style(hidden_cursor_style());

        textarea.set_block(Block::new().padding(Padding::uniform(MEASURE_PADDING)));
        textarea.set_wrap_mode(WrapMode::Word);

        let mut input = Self {
            textarea,
            block: Block::default().borders(Borders::ALL),
        };
        input.set_selected(false);
        input
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
                let text = get_text(&self.textarea);
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
        self.textarea.input(input);

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
        self.textarea.set_block(self.block.clone());
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
        self.textarea.set_block(self.block.clone());
    }

    /// Change appearance based on focus.
    ///
    /// No effect on input handling.
    fn set_focused(&mut self, focused: bool) {
        match focused {
            true => self.textarea.set_cursor_style(cursor_style()),
            false => self.textarea.set_cursor_style(hidden_cursor_style()),
        }
    }
}

impl<'s> HasTextarea<'s> for MeasureInput<'s> {
    fn textarea_mut(&mut self) -> &mut TextArea<'s> {
        &mut self.textarea
    }
}

impl<'a> Widget for &MeasureInput<'a> {
    fn render(self, rect: Rect, buffer: &mut Buffer) {
        self.textarea.render(rect, buffer);
    }
}

#[derive(Debug)]
struct Measure<'i> {
    id: u64,
    source: String,
    beats: Vec<BeatSpec>,
    sequence: Sequence,
    input: MeasureInput<'i>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
enum SampleManagerSelection {
    #[default]
    Files,
    Samples,
    /// Pop-up state not tracked here to simplify focus stack management. It gets much more messy
    /// when the stack cannot be copied.
    SampleNamePopup,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct SampleManager {
    selection: SampleManagerSelection,
    sample_index: usize,
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
    SampleManager(SampleManager),
}

impl Selection {
    /// Checks if there is any input focus (keys don't go to the main view)
    ///
    /// Implies that Esc should go back and only ^c will allow immediate termination.
    fn is_focused(&self) -> bool {
        match self {
            Self::App => false,
            Self::BPMInput { focus } | Self::Measure { focus, .. } => *focus,
            Self::SampleManager(_) => true,
        }
    }

    /// Checks if arrow navigation is enabled.
    fn is_navigable(&self) -> bool {
        match self {
            Self::App => true,
            Self::BPMInput { focus } | Self::Measure { focus, .. } => !focus,
            Self::SampleManager(SampleManager { selection, .. }) => match selection {
                SampleManagerSelection::Files => true,
                SampleManagerSelection::Samples => true,
                SampleManagerSelection::SampleNamePopup => false,
            },
        }
    }
}

#[derive(Debug, Clone)]
struct AppLayout {
    outer_rect: Rect,
    header_rect: Rect,
    upper_rect: Rect,
    lower_rect: Rect,
    action_rect: Rect,
}

#[derive(Debug, Clone, Copy)]
struct AppScrollyParams {
    measures: ScrollyParams,
    samples: ScrollyParams,
}

#[derive(Debug)]
pub struct App<'bpm, 'measure, 'filepicker, 'pop_up> {
    drumkick: MachineHandle,
    exit: bool,
    error: Option<Text<'static>>,
    debug_messages: VecDeque<String>,
    bpm_input: BPMInput<'bpm>,
    sample_picker: Filepicker<'filepicker>,
    /// Menu navigation and input focus
    selection: Vec<Selection>,
    next_measure_id: u64,
    measures_scrolly: Scrolly,
    samples_scrolly: Scrolly,
    measures: Vec<Measure<'measure>>,
    samples: IndexMap<String, Arc<LoadedSource>>,
    pop_up: Option<input_popup::InputPopup<'pop_up>>,
}

impl App<'_, '_, '_, '_> {
    pub fn new(opts: DrumkickOpts, init: InitialState) -> Result<Self, Box<dyn std::error::Error>> {
        let drumkick = MachineHandle::new(opts, init.clone())?;
        let input = BPMInput::new(drumkick.bpm());

        let mut sample_picker = Filepicker::new()?;
        for sample in init.samples.values() {
            if let Some(SourceOrigin::File(ref path)) = sample.origin {
                let file = File::from_absolute_path(path.clone())
                    .expect("app is passed real sample files");
                sample_picker.mark_file(file);
            }
        }

        let mut app = Self {
            drumkick,
            exit: false,
            error: None,
            debug_messages: VecDeque::new(),
            bpm_input: input,
            sample_picker,
            selection: vec![],
            next_measure_id: 0,
            measures_scrolly: Scrolly::new(),
            samples_scrolly: Scrolly::new(),
            measures: Vec::new(),
            samples: init.samples,
            pop_up: None,
        };
        app.add_measure(
            app.measures.len(),
            init.measure_beats.to_source(),
            init.measure_beats,
            Some(init.sequence),
            CursorMove::Head,
        );

        Ok(app)
    }

    pub fn run(
        &mut self,
        terminal: &mut DefaultTerminal,
    ) -> Result<(), Box<dyn std::error::Error>> {
        while !self.exit {
            self.propagate();
            let app_scrolly_params = self.calculate_scrolly_params(terminal.size()?);
            terminal.draw(|frame| self.draw(frame))?;

            let mut event_received = false;
            while !event_received {
                event_received = self.handle_events(app_scrolly_params)?;
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
    /// isn't mut - instead of the method being mut, it receives an additional mutable param. But
    /// does it mean that mutable sub-widgets (e.g. TextArea) need to be stored in that state?
    ///
    /// Anyway, this method resets all sub-widget state and then sets it according to focus.
    fn propagate(&mut self) {
        self.bpm_input.set_focused(false);
        self.bpm_input.set_selected(false);
        self.sample_picker.set_external_focus(false);
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
            Selection::SampleManager(SampleManager {
                selection: SampleManagerSelection::Files,
                ..
            }) => {
                self.sample_picker.set_external_focus(true);
            }
            _ => {}
        }
    }

    fn calculate_scrolly_params(&self, terminal_size: Size) -> AppScrollyParams {
        let (
            AppLayout {
                upper_rect,
                lower_rect,
                ..
            },
            _,
        ) = Self::app_level_layout(Rect::new(0, 0, terminal_size.width, terminal_size.height));

        let upper_height = upper_rect
            .height
            .saturating_sub(DEFAULT_BLOCK_PADDING * 2)
            .saturating_sub(1); // correct for top border
        //
        let lower_height = lower_rect
            .height
            .saturating_sub(DEFAULT_BLOCK_PADDING * 2)
            .saturating_sub(1); // correct for top border

        AppScrollyParams {
            measures: ScrollyParams {
                visible_height: upper_height,
                item_height: MEASURE_HEIGHT,
                item_count: self.measures.len(),
            },
            samples: ScrollyParams {
                visible_height: lower_height,
                item_height: 1,
                item_count: self.samples.len(),
            },
        }
    }

    fn general_instructions(&self) -> Line<'_> {
        let mut instructions: Vec<Span> = vec![" ".into()];

        let focused = self.selection().is_focused();

        if !focused {
            instructions.append(&mut vec![
                "Space".blue().bold(),
                (if self.drumkick.playing() {
                    ": Pause  "
                } else {
                    ": Play  "
                })
                .into(),
                "B/+/-".blue().bold(),
                ": Set BPM  ".into(),
                "A".blue().bold(),
                ": Add measure  ".into(),
                "S".blue().bold(),
                ": Samples  ".into(),
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
                header_rect: layout[0],
                upper_rect: layout[1],
                lower_rect: layout[2],
                action_rect: layout[3],
            },
            block,
        )
    }

    fn draw(&self, frame: &mut Frame) {
        let (
            AppLayout {
                outer_rect,
                header_rect,
                upper_rect,
                lower_rect,
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

        if let Selection::SampleManager(manager_selection) = self.selection() {
            frame.render_widget(
                Paragraph::new(Text::from("Sample manager").bold().centered())
                    .block(Block::new().padding(Padding::top(1))),
                header_rect,
            );
            self.draw_sample_picker(frame, upper_rect);
            self.draw_samples(frame, lower_rect, manager_selection);
        } else {
            frame.render_widget(&self.bpm_input, header_rect);
            self.draw_measures(frame, upper_rect);
            self.draw_info(frame, lower_rect);
        }

        if let Some(ref pop_up) = self.pop_up {
            pop_up.render(outer_rect, frame.buffer_mut());
        }

        self.draw_action_instructions(frame, action_rect);
    }

    fn draw_sample_picker(&self, frame: &mut Frame, rect: Rect) {
        let block = subview_block("Open sample files");
        let inner_rect = block.inner(rect);

        if let Some(ref error) = self.error {
            let layout = Layout::vertical(vec![
                Constraint::Fill(1),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .split(rect);
            let error_rect = layout[1];

            let error_block = Paragraph::new(error.clone());

            frame.render_widget(
                error_block.centered(),
                error_rect.centered(Constraint::Ratio(1, 2), Constraint::Ratio(1, 2)),
            );
        }

        frame.render_widget(&block, rect);
        self.sample_picker
            .render_ref(inner_rect, frame.buffer_mut());
    }

    fn draw_samples(&self, frame: &mut Frame, rect: Rect, sample_manager: SampleManager) {
        let title = format!(
            "{} loaded sample{}",
            self.samples.len(),
            naive_plural_suffix(self.samples.len()),
        );
        let block = subview_block(&title);

        frame.render_widget(&block, rect);
        let inner_rect = block.inner(rect);

        let scrolly_params = ScrollyParams {
            visible_height: inner_rect.height,
            item_height: 1,
            item_count: self.samples.len(),
        };

        self.samples_scrolly.render(
            frame,
            inner_rect,
            &scrolly_params,
            |scroll_buf, sample_rects| {
                let longest_label = self.samples.keys().map(|k| k.len()).max().unwrap_or(10);

                for (i, (&sample_rect, (label, sample))) in
                    sample_rects.iter().zip(self.samples.iter()).enumerate()
                {
                    let sample_layout = Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints([
                            Constraint::Max(longest_label as u16 + 2),
                            Constraint::Fill(2),
                            Constraint::Max(9),
                        ])
                        .split(sample_rect);

                    let label_rect = sample_layout[0];
                    let file_rect = sample_layout[1];
                    let khz_rect = sample_layout[2];

                    let backgound = if sample_manager.selection == SampleManagerSelection::Samples
                        && sample_manager.sample_index == i
                    {
                        Color::DarkGray
                    } else {
                        Color::default()
                    };

                    Text::from(label.as_str())
                        .bold()
                        .light_magenta()
                        .bg(backgound)
                        .render(label_rect, scroll_buf);

                    Text::from_iter(match sample.origin {
                        None => vec![Span::raw("unknown origin").light_red()],
                        Some(SourceOrigin::File(ref name)) => {
                            vec![Span::raw(name.to_string_lossy())]
                        }
                        Some(SourceOrigin::Builtin(ref name)) => {
                            vec![Span::raw("built-in:").gray(), " ".into(), Span::raw(name)]
                        }
                    })
                    .bg(backgound)
                    .render(file_rect, scroll_buf);

                    let sample_rate = sample.sample_rate as f64 / 1000.0;
                    Text::from(format!("{}kHz", sample_rate))
                        .bold()
                        .bg(backgound)
                        .render(khz_rect, scroll_buf);
                }
            },
        );
    }

    fn draw_measures(&self, frame: &mut Frame, rect: Rect) {
        let block = subview_block("Measures");

        frame.render_widget(&block, rect);
        let inner_rect = block.inner(rect);

        let upper_block_params = ScrollyParams {
            visible_height: inner_rect.height,
            item_height: MEASURE_HEIGHT,
            item_count: self.measures.len(),
        };

        self.measures_scrolly.render(
            frame,
            inner_rect,
            &upper_block_params,
            |scroll_buf, measures_list_layout| {
                let playback_progress = self.drumkick.playback_progress();

                for (&measure_rect, measure) in
                    measures_list_layout.iter().zip(self.measures.iter())
                {
                    let measure_layout = Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints([Constraint::Fill(1), Constraint::Max(7)])
                        .split(measure_rect);

                    measure.input.render(measure_layout[0], scroll_buf);

                    let measure_progress = playback_progress
                        .get(&measure.id)
                        .map(ToString::to_string)
                        .unwrap_or_else(|| "…".to_string());
                    let measure_progress = Paragraph::new(Text::from(measure_progress))
                        .block(Block::new().padding(MEASURE_PADDING.into()));

                    measure_progress.render(measure_layout[1], scroll_buf);
                }
            },
        );
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
            info_text.extend(error.clone());
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
        // Elements are inserted in order to preserve the same order of identical elements in
        // different modes. This is probably a bad idea because it leads to an excessive amount of
        // conditions.
        let mut line_elements: Vec<Span> = vec![];

        if self.selection().is_navigable() {
            line_elements.append(&mut vec!["Up/Down".bold(), ": Navigate".into()]);
        }

        let action_str = match self.selection() {
            Selection::BPMInput { focus: false } => "Edit BPM",
            Selection::BPMInput { focus: true } => "Apply BPM",
            Selection::Measure { focus: false, .. } => "Edit", // No "measure" to preserve space
            Selection::Measure { focus: true, .. } => "Apply measure",
            Selection::App => "",
            Selection::SampleManager(SampleManager {
                selection: SampleManagerSelection::Samples,
                ..
            }) => {
                if !self.samples.is_empty() {
                    "Rename sample"
                } else {
                    ""
                }
            }
            Selection::SampleManager(SampleManager {
                selection: SampleManagerSelection::SampleNamePopup,
                ..
            }) => "Apply",
            Selection::SampleManager(_) => "",
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
                "  ".into(),
                "SHFT↕".bold(),
                ": Move".into(),
                "  ".into(),
                "C".bold(),
                ": Clone".into(),
            ]);
        }

        if let Selection::SampleManager(SampleManager {
            selection: SampleManagerSelection::Files,
            ..
        }) = self.selection()
        {
            line_elements.append(&mut vec![
                "  ".into(),
                "Space".bold(),
                ": ".into(),
                (if self.sample_picker.current_file_marked() {
                    "Remove sample"
                } else {
                    "Add sample"
                })
                .into(),
            ]);
        }

        if matches!(
            self.selection(),
            Selection::Measure { focus: false, .. }
                | Selection::SampleManager(SampleManager {
                    selection: SampleManagerSelection::Samples,
                    ..
                })
        ) && !self.samples.is_empty()
        {
            line_elements.append(&mut vec!["  ".into(), "X".bold(), ": Delete".into()]);
        }

        if self.selection().is_focused() {
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

    fn handle_events(&mut self, mut scrolly_params: AppScrollyParams) -> io::Result<bool> {
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
                // Update size on resize
                if let Resize(width, height) = event
                    && let Selection::Measure { index, .. } = self.selection()
                {
                    scrolly_params = self.calculate_scrolly_params(Size::new(width, height));
                    self.measures_scrolly
                        .adjust_scroll_offset_for_selection(scrolly_params.measures, index);
                    if let Selection::SampleManager(SampleManager { sample_index, .. }) =
                        self.selection()
                    {
                        self.samples_scrolly.adjust_scroll_offset_for_selection(
                            scrolly_params.samples,
                            sample_index,
                        );
                    }
                }

                if let Event::Key(key_event) = event {
                    self.handle_global_key_event(key_event)
                }

                match self.selection() {
                    Selection::BPMInput { focus: true } => self.handle_bpm_input_key_event(event),
                    Selection::Measure {
                        focus: true, index, ..
                    } => self.handle_measure_input_key_event(index, event),
                    Selection::SampleManager(manager_selection) => self
                        .handle_sample_manager_event(
                            manager_selection,
                            event,
                            scrolly_params.samples,
                        ),
                    _ => self.handle_normal_event(event, scrolly_params),
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

    fn handle_normal_event(&mut self, event: Event, scrolly_params: AppScrollyParams) {
        // Passing `scrolly_params` could be avoided if this returned an action instead of
        // performing it.
        if let Event::Key(key_event) = event {
            match (key_event.code, key_event.modifiers) {
                (Char('q'), _) => self.exit(),
                (Char(' '), _) => self.toggle_playing(),
                (Char('+') | Char('='), _) => self.change_bpm(5.0),
                (Char('-') | Char('_'), _) => self.change_bpm(-5.0),
                (Char('b'), _) => {
                    self.selection.push(Selection::BPMInput { focus: true });
                }
                // Move measure
                (key @ (KeyCode::Up | KeyCode::Down), KeyModifiers::SHIFT) => {
                    let step = match key {
                        KeyCode::Up => -1,
                        KeyCode::Down => 1,
                        _ => unreachable!(),
                    };
                    if let Selection::Measure {
                        index: prev_index, ..
                    } = self.selection()
                    {
                        let new_index = self.navigate_measure_only_selection(
                            ScrollyNav::Step(step),
                            scrolly_params.measures,
                        );
                        self.measures.swap(prev_index, new_index);
                    }
                }
                (KeyCode::Up, _) => self.navigate_selection(-1, scrolly_params.measures),
                (KeyCode::Down, _) => self.navigate_selection(1, scrolly_params.measures),
                (KeyCode::PageUp, _) => {
                    self.navigate_measure_only_selection(
                        ScrollyNav::PageUp,
                        scrolly_params.measures,
                    );
                }
                (KeyCode::PageDown, _) => {
                    self.navigate_measure_only_selection(
                        ScrollyNav::PageDown,
                        scrolly_params.measures,
                    );
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
                    _ => {}
                },
                (Char('a'), _) => {
                    let new_index = self.add_measure(
                        self.measures.len(),
                        String::from("1:"),
                        vec![],
                        None,
                        CursorMove::End,
                    );
                    self.replace_selection(Selection::Measure {
                        index: new_index,
                        focus: true,
                    });
                    self.measures_scrolly
                        .adjust_scroll_offset_for_selection(scrolly_params.measures, new_index);
                }
                (Char('c'), _) => {
                    if let Selection::Measure { index, .. } = self.selection() {
                        let new_index = self.clone_measure(index);
                        self.measures_scrolly
                            .adjust_scroll_offset_for_selection(scrolly_params.measures, new_index);
                    }
                }
                (Char('x'), _) => {
                    if let Selection::Measure { index, .. } = self.selection() {
                        let maybe_new_index = self.remove_measure(index);
                        if let Some(new_index) = maybe_new_index {
                            self.measures_scrolly.adjust_scroll_offset_for_selection(
                                scrolly_params.measures,
                                new_index,
                            );
                        }
                    }
                }
                (Char('m'), _) => {
                    if let Selection::Measure { index, .. } = self.selection() {
                        self.mute_unmute_measure(index);
                    }
                }
                (Char('s'), _) => self.selection.push(Selection::SampleManager(SampleManager {
                    selection: SampleManagerSelection::default(),
                    sample_index: 0,
                })),

                _ => {}
            };
        }
    }

    fn handle_bpm_input_key_event(&mut self, event: Event) {
        let effect = self.bpm_input.handle_event(event);
        match effect {
            InputEditEffect::Editing => {}
            InputEditEffect::Aborted => {
                self.bpm_input
                    .set_textarea_text(self.drumkick.bpm().to_string());
                self.selection.pop();
            }
            InputEditEffect::Applied(bpm) => {
                self.set_bpm(bpm);
                self.selection.pop();
            }
            InputEditEffect::Error(err) => {
                self.set_error_str(error_text(&err));
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
                measure.input.set_textarea_text(&measure.source);
                self.clear_error();

                self.replace_selection(Selection::Measure {
                    index,
                    focus: false,
                });
            }
            InputEditEffect::Applied((source, beat_specs)) => {
                match samples::arrange_samples_into_sequence_items(&beat_specs, &self.samples) {
                    Err(err) => self.set_error_text(error_text(&err.to_string())),
                    Ok(items) => {
                        measure.source = source;
                        measure.sequence.items = items;
                        measure.beats = beat_specs;
                        self.update_sequences();
                        self.clear_error();
                        self.selection.pop();
                    }
                }
            }
            InputEditEffect::Error(err) => {
                self.set_error_text(error_text(&err.to_string()));
            }
        }
    }

    fn handle_sample_manager_event(
        &mut self,
        manager_selection: SampleManager,
        event: Event,
        scrolly_params: ScrollyParams,
    ) {
        if let Event::Key(key_event) = event {
            match manager_selection.selection {
                SampleManagerSelection::Files => match (key_event.code, key_event.modifiers) {
                    (KeyCode::Char('q'), _) => {
                        self.selection.pop();
                    }
                    (KeyCode::Tab, _) => {
                        self.replace_selection(Selection::SampleManager(SampleManager {
                            selection: SampleManagerSelection::Samples,
                            ..manager_selection
                        }));
                    }
                    _ => {
                        let effect = self.sample_picker.handle_event(event);
                        self.clear_error();

                        match effect {
                            FilepickerEffect::Picking => {}
                            FilepickerEffect::Closed => {
                                self.selection.pop();
                            }
                            FilepickerEffect::Added(file) => {
                                if !self.has_sample(&file.path) {
                                    let resp = self.add_sample(&file.path);
                                    if resp.is_err() {
                                        self.set_error_str(format!(
                                            "Could not load sample: {}",
                                            file.path.file_name().map_or_else(
                                                || file.path.to_string_lossy(),
                                                |n| n.to_string_lossy(),
                                            )
                                        ));
                                    } else {
                                        self.sample_picker.mark_file(file);
                                    }
                                }
                            }
                            FilepickerEffect::Removed(path) => {
                                if let Err(err) = self.remove_sample_by_path(&path) {
                                    self.set_error_str(err);
                                } else {
                                    self.sample_picker.unmark_file(&path);
                                    self.fix_sample_selection_index(manager_selection);
                                }
                            }
                            FilepickerEffect::Error(err) => {
                                self.set_error_str(err);
                            }
                        }
                    }
                },
                SampleManagerSelection::Samples => match (key_event.code, key_event.modifiers) {
                    (KeyCode::Char('q'), _) => {
                        self.selection.pop();
                    }
                    (KeyCode::Tab, _) => {
                        self.replace_selection(Selection::SampleManager(SampleManager {
                            selection: SampleManagerSelection::Files,
                            ..manager_selection
                        }));
                    }
                    (KeyCode::Up, _) => {
                        let outcome = self.samples_scrolly.navigate(
                            scrolly_params,
                            manager_selection.sample_index,
                            ScrollyNav::Step(-1),
                        );
                        self.set_sample_index(outcome.index);
                    }
                    (KeyCode::Down, _) => {
                        let outcome = self.samples_scrolly.navigate(
                            scrolly_params,
                            manager_selection.sample_index,
                            ScrollyNav::Step(1),
                        );
                        self.set_sample_index(outcome.index);
                    }
                    (KeyCode::PageUp, _) => {
                        let outcome = self.samples_scrolly.navigate(
                            scrolly_params,
                            manager_selection.sample_index,
                            ScrollyNav::PageUp,
                        );
                        self.set_sample_index(outcome.index);
                    }
                    (KeyCode::PageDown, _) => {
                        let outcome = self.samples_scrolly.navigate(
                            scrolly_params,
                            manager_selection.sample_index,
                            ScrollyNav::PageDown,
                        );
                        self.set_sample_index(outcome.index);
                    }
                    (KeyCode::Char('x'), _) if !self.samples.is_empty() => {
                        if let Err(err) =
                            self.remove_sample_by_index(manager_selection.sample_index)
                        {
                            self.set_error_str(err);
                        } else {
                            self.fix_sample_selection_index(manager_selection);
                        }
                    }
                    (KeyCode::Enter, _) if !self.samples.is_empty() => {
                        self.pop_up = Some(InputPopup::new(
                            String::from("Edit sample name"),
                            [Constraint::Max(50), Constraint::Max(5)],
                            self.samples
                                .get_index_entry(manager_selection.sample_index)
                                .expect("sample entry by index")
                                .key()
                                .as_str(),
                            |name| {
                                validate_sample_name(name).map_err(|e| {
                                    let after_err = (e.position + 1).clamp(0, name.len());
                                    let message_span = Span::from(e.message).light_red();

                                    let mut line = Line::default();

                                    if !name.is_empty() {
                                        line.extend([
                                            Span::from(name[0..e.position].to_string()).green(),
                                            Span::from(name[e.position..after_err].to_string())
                                                .bold()
                                                .underlined()
                                                .light_red(),
                                            Span::from(name[after_err..name.len()].to_string())
                                                .green(),
                                            Span::from(": "),
                                        ]);
                                    }

                                    line.push_span(message_span);
                                    line
                                })
                            },
                        ));

                        self.selection.push(Selection::SampleManager(SampleManager {
                            selection: SampleManagerSelection::SampleNamePopup,
                            ..manager_selection
                        }));
                    }
                    (KeyCode::Esc, _) => {
                        self.selection.pop();
                    }
                    _ => {}
                },
                SampleManagerSelection::SampleNamePopup => {
                    let mut pop_up = self.pop_up.take().expect("pop-up initialised");

                    self.pop_up = match pop_up.handle_event(event) {
                        InputEditEffect::Editing => Some(pop_up),
                        InputEditEffect::Aborted => {
                            self.selection.pop();
                            self.clear_error();
                            None
                        }
                        InputEditEffect::Applied(new_name) => {
                            match self.rename_sample(manager_selection.sample_index, &new_name) {
                                Ok(_) => {
                                    self.selection.pop();
                                    self.clear_error();
                                    None
                                }
                                Err(err) => {
                                    self.set_error_str(err);
                                    Some(pop_up)
                                }
                            }
                        }
                        InputEditEffect::Error(err) => {
                            self.set_error_text(err);
                            Some(pop_up)
                        }
                    };
                }
            }
        }
    }

    fn set_error_text(&mut self, error: impl Into<Text<'static>>) {
        self.error = Some(error.into());
    }

    fn set_error_str(&mut self, error: impl ToString) {
        self.error = Some(Text::from(error.to_string()).light_red());
    }

    fn clear_error(&mut self) {
        self.error = None;
    }

    fn exit(&mut self) {
        if let Err(err) = self.drumkick.stop() {
            self.set_error_str(err);
        }
        self.exit = true;
    }

    fn toggle_playing(&mut self) {
        let result = self.drumkick.toggle_playing();

        if let Err(err) = result {
            self.set_error_str(err);
        } else {
            self.clear_error();
        };
    }

    fn change_bpm(&mut self, diff: f64) {
        self.set_bpm(self.drumkick.bpm() + diff)
    }

    fn set_bpm(&mut self, new_bpm: f64) {
        self.drumkick.set_bpm(new_bpm);
        self.bpm_input
            .set_textarea_text(self.drumkick.bpm().to_string());
    }

    /// App-level cursor nav which can step between BPM input, app, and measures focus.
    fn navigate_selection(&mut self, step: isize, upper_block_params: ScrollyParams) {
        let (start_index, corrected_step) = match self.selection() {
            Selection::BPMInput { .. } => (0, step - 2),
            Selection::App => (0, step - 1),
            Selection::Measure { index, .. } => (index, step),
            Selection::SampleManager(_) => {
                debug_assert!(
                    false,
                    "sample manager variant must be unreachable from top-level cursor nav impl"
                );
                return;
            }
        };

        let scroll_outcome = self.measures_scrolly.navigate(
            upper_block_params,
            start_index,
            ScrollyNav::Step(corrected_step),
        );

        let new_active_measure = match scroll_outcome.condition {
            NavCondition::Underflow(underflow) => {
                if underflow == 1 {
                    Selection::App
                } else {
                    Selection::BPMInput { focus: false }
                }
            }
            _ => Selection::Measure {
                index: scroll_outcome.index,
                focus: false,
            },
        };

        self.replace_selection(new_active_measure);
    }

    /// Navigate up/down within the measures block.
    fn navigate_measure_only_selection(
        &mut self,
        nav: ScrollyNav,
        upper_block_params: ScrollyParams,
    ) -> usize {
        // Without measures we've no business
        if self.measures.is_empty() {
            return 0;
        }

        let index = match self.selection() {
            Selection::Measure { index, .. } => index,
            _ => 0,
        };
        // Page Down but measures are not selected - select measure 0 before

        let scroll_outcome = self
            .measures_scrolly
            .navigate(upper_block_params, index, nav);

        self.replace_selection(Selection::Measure {
            index: scroll_outcome.index,
            focus: false,
        });

        scroll_outcome.index
    }

    fn set_sample_index(&mut self, new_index: usize) {
        self.replace_selection(Selection::SampleManager(SampleManager {
            selection: SampleManagerSelection::Samples,
            sample_index: new_index,
        }))
    }

    fn add_measure(
        &mut self,
        index: usize,
        source: String,
        beats: Vec<BeatSpec>,
        sequence: Option<Sequence>,
        cursor_pos: CursorMove,
    ) -> usize {
        let input = MeasureInput::new(&source, cursor_pos);

        self.measures.insert(
            index,
            Measure {
                id: self.next_measure_id,
                source,
                beats,
                sequence: sequence.unwrap_or_else(|| Sequence::new([])),
                input,
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
        let DataCursor(row, col) = existing_measure.input.textarea.cursor();
        let new_index = index + 1;
        self.add_measure(
            new_index,
            existing_measure.source.clone(),
            existing_measure.beats.clone(),
            Some(existing_measure.sequence.clone()),
            CursorMove::Jump(row as u16, col as u16),
        )
    }

    fn remove_measure(&mut self, index: usize) -> Option<usize> {
        self.measures.remove(index);
        let measure_count = self.measures.len();
        let maybe_new_index = if measure_count > 0 {
            let new_index = index.min(self.measures.len() - 1);
            self.replace_selection(Selection::Measure {
                index: new_index,
                focus: false,
            });
            Some(new_index)
        } else {
            self.replace_selection(Selection::App);
            None
        };

        self.update_sequences();

        maybe_new_index
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

    fn has_sample(&self, path: &Path) -> bool {
        self.samples
            .values()
            .find(|&sample| {
                if let Some(SourceOrigin::File(ref file_path)) = sample.origin {
                    file_path == path
                } else {
                    false
                }
            })
            .is_some()
    }

    fn add_sample(&mut self, path: &Path) -> Result<(), SymphoniaError> {
        let sound_data = load_from_path(path)?;
        let mut name = path
            .with_extension("")
            .file_name()
            .map(|os_str| clean_sample_name(&os_str.to_string_lossy()))
            .unwrap_or_else(|| String::from("newsample"));

        // deconflict name
        let name_len = name.len();
        let mut suffix_i: usize = 2;
        let mut suffix = NumBuffer::new();
        while self.samples.contains_key(&name) {
            name.truncate(name_len);
            name.push_str(suffix_i.format_into(&mut suffix));
            suffix_i += 1;
        }

        self.samples.insert(name, sound_data);

        self.update_sequences();
        self.clear_error();
        Ok(())
    }

    fn fix_sample_selection_index(&mut self, prev_manager_selection: SampleManager) {
        let last_sample_index = self.samples.len().saturating_sub(1);
        if prev_manager_selection.sample_index > last_sample_index {
            self.replace_selection(Selection::SampleManager(SampleManager {
                sample_index: last_sample_index,
                ..prev_manager_selection
            }));
        }
    }

    fn remove_sample_by_index(&mut self, index: usize) -> Result<(), String> {
        if let Some((name, _)) = self.samples.get_index(index) {
            self.remove_sample(&name.clone())
        } else {
            Err(format!("Invalid sample index {index}"))
        }
    }

    fn remove_sample_by_path(&mut self, path: &Path) -> Result<(), String> {
        if let Some((name, _)) = self.samples.iter().find(|(_name, source)| {
            if let Some(SourceOrigin::File(ref source_path)) = source.origin {
                source_path == path
            } else {
                false
            }
        }) {
            self.remove_sample(&name.clone())
        } else {
            Err(format!("File {path:?} not used by any sample"))
        }
    }

    fn find_measures_using_sample(&self, name: &str) -> impl Iterator<Item = usize> {
        self.measures
            .iter()
            .enumerate()
            .filter(move |(_i, measure)| {
                measure
                    .beats
                    .iter()
                    .find(|beat_spec| beat_spec.samples.iter().any(|s| s == name))
                    .is_some()
            })
            .map(|(i, _)| i)
    }

    fn remove_sample(&mut self, name: &str) -> Result<(), String> {
        let measures_using_sample_base_1: Vec<_> = self
            .find_measures_using_sample(name)
            .map(|i| (i + 1).to_string())
            .collect();

        if measures_using_sample_base_1.is_empty() {
            if let Some(SourceOrigin::File(path)) = self
                .samples
                .shift_remove(name)
                .as_ref()
                .and_then(|s| s.origin.as_ref())
            {
                self.sample_picker.unmark_file(path);
            }

            self.update_sequences();
            self.clear_error();
            Ok(())
        } else {
            Err(format!(
                "Sample is used by measure{} {}.",
                naive_plural_suffix(measures_using_sample_base_1.len()),
                measures_using_sample_base_1.join(","),
            ))
        }
    }

    fn rename_sample(&mut self, index: usize, new_name: &str) -> Result<(), String> {
        let old_name = self
            .samples
            .replace_index(index, new_name.to_string())
            .map_err(|_| format!("There is already a sample called {new_name}"))?;

        let measures_using_sample: Vec<_> = self.find_measures_using_sample(&old_name).collect();

        for measure_index in measures_using_sample {
            let measure = self
                .measures
                .get_mut(measure_index)
                .expect("measure by index");

            for beat in measure.beats.iter_mut() {
                for sample_name in beat.samples.iter_mut() {
                    if *sample_name == *old_name {
                        sample_name.clear();
                        sample_name.push_str(new_name)
                    }
                }
            }

            measure.source = measure.beats.to_source();
            measure.input.set_textarea_text(&measure.source);
        }

        Ok(())
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
