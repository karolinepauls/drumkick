mod cli;
mod parsing;
mod term;
mod tui;
use clap::{Parser, builder::RangedU64ValueParser, value_parser};
use indexmap::IndexMap;

use std::collections::HashSet;
use std::fmt;
use thiserror::Error;

use drumkick::{
    BUFFER_SIZE, DrumkickOpts, PRECISION, Track,
    embedded_samples::{EMBEDDED_SAMPLE_NAMES, get_embedded_sample_names},
    measure_grammar::{BEAT_FRACTIONS, BeatSpec, MeasureParseError, backtick_quote, parse_measure},
    pest_set_error_detail,
    samples::load_samples,
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

    #[arg(long, default_value_t = false, help = "Run without a TUI")]
    headless: bool,
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

fn init() -> Result<(Args, DrumkickOpts, Track), Box<dyn std::error::Error>> {
    let args = Args::parse();
    pest_set_error_detail(true);

    let track_spec =
        unwrap_or_terminate(interpret_args(&args), ExitCodes::CantInterpretArgs.code());

    let beats = unwrap_or_terminate(
        load_samples(&track_spec.measure_beats, &track_spec.sample_locations),
        ExitCodes::CantLoadSamples.code(),
    );

    let opts = DrumkickOpts {
        buffer_size: args.buffer_size,
        precision: args.precision,
        sound_capacity: 512,
        message_buffer_size: 32,
    };
    let track = Track {
        bpm: track_spec.bpm,
        measure_beats: beats,
    };

    Ok((args, opts, track))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (args, opts, track) = init()?;
    eprintln!("Measure has {} beats", track.beats_in_measure());

    if args.headless {
        cli::run_cli(opts, track)
    } else {
        tui::run_tui(opts, track)
    }
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
