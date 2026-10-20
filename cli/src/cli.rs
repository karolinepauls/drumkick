use crate::parsing;
use crate::term;
use crossterm::{
    cursor::{Hide, MoveRight, MoveToColumn, Show},
    event::{
        Event,
        KeyCode::{self, Char},
        KeyEvent, KeyModifiers,
    },
    execute,
    style::{Attribute, Color, PrintStyledContent, StyledContent, Stylize},
    terminal::{Clear, ClearType, disable_raw_mode, enable_raw_mode},
};
use std::fmt::{self, Write as _};
use std::io;
use std::time;

use drumkick::{
    BeatNo, DrumkickOpts, InitialState, MachineHandle, PlaybackProgress, Response, ResponseLevel,
};

use std::panic;

const UI_POLL_DURATION: time::Duration = time::Duration::from_millis(100);
const BPM_LABEL: &str = "BPM: ";

fn read_key() -> io::Result<Option<KeyEvent>> {
    let event = term::read_event_with_timeout(UI_POLL_DURATION)?;

    if let Some(Event::Key(key_event)) = event {
        Ok(Some(key_event))
    } else {
        Ok(None)
    }
}

fn help_label_style(label: &str) -> StyledContent<&str> {
    label.with(Color::White).attribute(Attribute::Bold)
}

fn help_key_style(label: &str) -> StyledContent<&str> {
    label.with(Color::Blue).attribute(Attribute::Bold)
}

fn clean_up_terminal() -> io::Result<()> {
    execute!(io::stderr(), Show)?;
    disable_raw_mode()
}

fn overwrite_string(input: &mut String, bpm: impl fmt::Display) -> fmt::Result {
    input.clear();
    write!(input, "{}", bpm)
}

fn clear_line() -> io::Result<()> {
    execute!(io::stderr(), Clear(ClearType::CurrentLine), MoveToColumn(0))
}

fn print_response(response: &Response) -> io::Result<()> {
    let label = format!("{:?}", response.level);
    let (label_colour, msg_colour) = match response.level {
        ResponseLevel::User => (Color::Blue, Color::Reset),
        ResponseLevel::Debug => (Color::DarkGrey, Color::DarkGrey),
    };
    let msg = format!(": {}\r\n", response.message);

    execute!(
        io::stderr(),
        MoveToColumn(0),
        PrintStyledContent(label.with(label_colour)),
        PrintStyledContent(msg.with(msg_colour)),
    )
}

/// Draw BPM input line with a fake cursor (park the real curor at the beginning)
///
/// The line is short enough to be overwritten by any realistic `dbg!()` or panic output.
fn print_bpm_input_line(input: &str, playing: bool, progress: &PlaybackProgress) -> io::Result<()> {
    let max_progress = progress.values().max().unwrap_or(&BeatNo::ONE);
    let progress = format!("{max_progress:7}");
    execute!(
        io::stderr(),
        Clear(ClearType::CurrentLine),
        MoveToColumn(0),
        //BPM line
        PrintStyledContent(progress.with(Color::White).attribute(Attribute::Bold)),
        PrintStyledContent(BPM_LABEL.with(Color::Yellow).attribute(Attribute::Bold)),
        PrintStyledContent(input.attribute(Attribute::Bold)),
        PrintStyledContent("▒".on(Color::DarkGrey).attribute(Attribute::RapidBlink)),
        MoveRight(1),
        PrintStyledContent(
            if playing { "" } else { "-- PAUSED --" }
                .with(Color::DarkMagenta)
                .attribute(Attribute::Bold),
        ),
        // Move cursor to BPM line's input position
        MoveToColumn(0),
    )
}

fn print_help_line() -> io::Result<()> {
    let newline = PrintStyledContent("\r\n".with(Color::Reset));
    execute!(
        io::stderr(),
        Clear(ClearType::CurrentLine),
        MoveToColumn(0),
        PrintStyledContent(
            "Keys:"
                .with(Color::Black)
                .on(Color::White)
                .attribute(Attribute::Bold)
        ),
        MoveRight(1),
        PrintStyledContent(help_label_style(" Play/Pause: ")),
        PrintStyledContent(help_key_style("<SPACE>")),
        PrintStyledContent(help_label_style(" Adjust BPM: ")),
        PrintStyledContent(help_key_style("<+/-/Up/Down>")),
        PrintStyledContent(help_label_style(" Apply BPM: ")),
        PrintStyledContent(help_key_style("<Enter>")),
        PrintStyledContent(help_label_style(" Quit: ")),
        PrintStyledContent(help_key_style("<Q/^C>")),
        newline,
    )
}

pub fn run_cli(opts: DrumkickOpts, track: InitialState) -> Result<(), Box<dyn std::error::Error>> {
    enable_raw_mode()?;
    term::enable_output_postprocessing()?;
    execute!(io::stderr(), Hide)?;

    let default_panic = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        clean_up_terminal().expect("terminal cleanup on panic exit");
        default_panic(info);
    }));

    let mut drumkick = MachineHandle::new(opts, track)?;
    drumkick.play()?;
    let mut input = drumkick.bpm().to_string();

    print_help_line()?;
    let mut maybe_terminal_event: Option<KeyEvent> = None;
    loop {
        for resp in drumkick.iter_resposes() {
            print_response(&resp)?;
        }

        print_bpm_input_line(
            input.as_str(),
            drumkick.playing(),
            &drumkick.playback_progress(),
        )?;

        if let Some(event) = maybe_terminal_event {
            match (event.code, event.modifiers) {
                (Char(' '), _) => {
                    drumkick.toggle_playing()?;
                }
                (Char('c'), KeyModifiers::CONTROL) | (Char('q'), _) => {
                    clear_line()?;
                    drumkick.stop()?;
                    break;
                }
                (Char('+') | Char('=') | KeyCode::Up, _) => {
                    drumkick.set_bpm(drumkick.bpm() + 5.0);
                    overwrite_string(&mut input, drumkick.bpm())?;
                }
                (Char('-') | Char('_') | KeyCode::Down, _) => {
                    drumkick.set_bpm(drumkick.bpm() - 5.0);
                    overwrite_string(&mut input, drumkick.bpm())?;
                }
                (Char(ch @ '0'..='9') | Char(ch @ '.'), _) => {
                    input.push(ch);
                }
                (KeyCode::Backspace, _) => {
                    input.pop();
                }
                (KeyCode::Enter, _) => match parsing::parse_bpm(&input) {
                    Ok(new_bpm) => {
                        drumkick.set_bpm(new_bpm);
                        overwrite_string(&mut input, drumkick.bpm())?;
                        clear_line()?;
                    }
                    Err(mut msg) => {
                        msg.push_str("\r\n");
                        execute!(
                            io::stderr(),
                            Clear(ClearType::CurrentLine),
                            MoveToColumn(0),
                            PrintStyledContent(msg.with(Color::Red).attribute(Attribute::Bold)),
                        )?;
                    }
                },
                _ => {
                    print_help_line()?;
                }
            }
        }
        maybe_terminal_event = read_key()?;
    }

    clean_up_terminal()?;
    Ok(())
}
