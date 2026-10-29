mod cli;
mod parsing;
mod term;
mod tui;
use clap::{
    Parser,
    builder::{OsStringValueParser, TypedValueParser},
    value_parser,
};
use indexmap::IndexMap;
use std::path::{Component, Path, PathBuf, absolute};

use std::collections::HashSet;
use std::ffi::OsStr;
use std::fmt;
use thiserror::Error;

use drumkick::{
    BUFFER_SIZE, DrumkickOpts, InitialState,
    embedded_samples::{EMBEDDED_SAMPLE_NAMES, get_embedded_sample_names},
    measure_grammar::{BEAT_FRACTIONS, BeatSpec, MeasureParseError, backtick_quote, parse_measure},
    pest_set_error_detail, samples,
};

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

/// Resolves ".." without resolving symlinks.
fn normalise_curdir_and_parent_dir(path: &Path) -> PathBuf {
    let mut new_path = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                new_path.pop();
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => new_path.push(comp),
        }
    }
    new_path
}

/// Breaks a "name=path" string into parts.
fn parse_pair(s: &OsStr) -> Result<(String, PathBuf), String> {
    let bytes = s.as_encoded_bytes();
    let i = bytes.iter().position(|&b| b == b'=').ok_or_else(|| {
        format!(
            "expected name=path, got {}",
            backtick_quote(&s.to_string_lossy())
        )
    })?;

    let name = std::str::from_utf8(&bytes[..i]).map_err(|e| e.to_string())?;
    // Unfortunately, there is no safe and portable way to build a path from bytes.
    let path = unsafe { OsStr::from_encoded_bytes_unchecked(&bytes[i + 1..]) };

    let abspath = absolute(path).map_err(|e| e.to_string())?;
    let normalised = normalise_curdir_and_parent_dir(&abspath);
    Ok((name.to_owned(), normalised))
}

#[derive(Parser, Debug)]
#[command(version, about, long_about = None, next_line_help = true)]
struct Args {
    #[arg(
        default_value_t = "1:CLICK 2:click 3:click 4:click".to_string(),
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
This will result in the `kick` sample being available in the `MEASURE` argument.",
        value_parser = OsStringValueParser::new().try_map(|s| parse_pair(&s)),
    )]
    samples: Vec<(String, PathBuf)>,

    #[arg(short, long, default_value_t = 60.0)]
    bpm: f64,

    #[arg(
        long,
        default_value_t = BUFFER_SIZE,
        value_parser = value_parser!(u32).range(1..),
        help = "Audio host buffer size in audio wave samples (lower → better responsiveness, more CPU).
This setting may be ignored by the system audio server. Extreme values, unless ignored, may result in the app
being given insufficient time to produce the audio wave, resulting in buffer underruns and clicks/pops/silence."
    )]
    buffer_size: u32,

    #[arg(long, default_value_t = false, help = "Run without a TUI")]
    headless: bool,
}

#[derive(Debug)]
struct TrackSpec<'a> {
    bpm: f64,
    measure_beats: Vec<BeatSpec>,
    sample_locations: IndexMap<&'a str, &'a Path>,
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

fn init() -> Result<(Args, DrumkickOpts, InitialState), Box<dyn std::error::Error>> {
    let args = Args::parse();
    pest_set_error_detail(true);

    let track_spec =
        unwrap_or_terminate(interpret_args(&args), ExitCodes::CantInterpretArgs.code());

    let samples = unwrap_or_terminate(
        samples::load_samples(&track_spec.sample_locations),
        ExitCodes::CantLoadSamples.code(),
    );

    let sequence = unwrap_or_terminate(
        samples::load_samples_into_sequence(&track_spec.measure_beats, &samples),
        ExitCodes::CantLoadSamples.code(),
    );

    let opts = DrumkickOpts {
        buffer_size: args.buffer_size,
        sound_capacity: 512,
        message_buffer_size: 32,
    };
    let track = InitialState {
        bpm: track_spec.bpm,
        samples,
        measure_beats: track_spec.measure_beats,
        sequence,
    };

    Ok((args, opts, track))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (args, opts, initial_state) = init()?;
    eprintln!("Measure has {} beats", initial_state.sequence.beat_count());

    if args.headless {
        cli::run_cli(opts, initial_state)
    } else {
        tui::run_tui(opts, initial_state)
    }
}

fn interpret_args<'a>(args: &'a Args) -> Result<TrackSpec<'a>, Box<ArgsParseError>> {
    let sample_locations = IndexMap::from_iter(
        args.samples
            .iter()
            .map(|(name, path)| (name.as_str(), path.as_path())),
    );

    // Build a vec of all sample names, embedded and provided, for error reporting.
    let mut all_sample_names = get_embedded_sample_names();
    let mut seen_sample_names: HashSet<_> = all_sample_names.iter().copied().collect();

    // Sample locations may have failed to load, in which case we won't append them to all_sample_names.
    for key in sample_locations.keys() {
        let new = seen_sample_names.insert(key);
        if new {
            all_sample_names.push(key);
        }
    }

    let measure_beats =
        parse_measure(&args.measure, &all_sample_names).map_err(|e| ArgsParseError {
            sample_locations: None,
            measure_parse_errors: Some(e),
        })?;

    Ok(TrackSpec {
        bpm: args.bpm,
        measure_beats,
        sample_locations,
    })
}

#[cfg(test)]
mod test {
    use super::*;
    use rstest::*;

    #[rstest]
    #[case(PathBuf::from("/abc/../def"), PathBuf::from("/def"))]
    #[case::parent_dir_of_root_dir_is_still_root_dir(
        PathBuf::from("/../def"),
        PathBuf::from("/def")
    )]
    #[case(PathBuf::from("/abc/def/ghi/../../"), PathBuf::from("/abc"))]
    #[case(PathBuf::from("/abc/./def/"), PathBuf::from("/abc/def"))]
    #[case(PathBuf::from("./abc/"), PathBuf::from("abc"))]
    fn test_normalise_curdir_and_parent_dir(#[case] input: PathBuf, #[case] expected: PathBuf) {
        assert_eq!(normalise_curdir_and_parent_dir(&input), expected);
    }
}
