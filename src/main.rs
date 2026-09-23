use clap::{Parser, builder::RangedU64ValueParser, value_parser};
use kira::{sound::FromFileError, sound::static_sound::StaticSoundData};
use pest::set_error_detail;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::io::{self, Read};
use std::{thread, time};
use thiserror::Error;

use drumkick::{
    Beat, DrumkickBackend, DrumkickOpts, Message, Track,
    embedded_samples::{EMBEDDED_SAMPLE_NAMES, get_embedded_sample, get_embedded_sample_names},
    measure_grammar::{
        BEAT_FRACTIONS, BeatSpec, MeasureParseError, backtick_quote, join_backtick_quoted,
        parse_measure,
    },
};

const BUFFER_SIZE: u32 = 2048;
const PRECISION: usize = 32;

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
    beats_per_minute: f64,
    measure_beats: Vec<BeatSpec>,
    ordered_samples: Vec<&'a str>,
    sample_locations: HashMap<&'a str, &'a str>,
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

#[derive(Error, Debug)]
enum SampleLoadingError {
    #[error("no such sample - available sample names are: {}", join_backtick_quoted(.0.iter().map(String::as_ref)))]
    NoSuchSample(Vec<String>),
    #[error("sample loading failed {0}")]
    FromFile(FromFileError),
}

#[derive(Error, Debug)]
#[error(
    "error loading samples: {}",
    .errors.iter().map(|(sample, err)| format!("{:?}: {}", sample, err)).collect::<Vec<_>>().join(", ")
)]
struct SampleLoadingErrors {
    errors: Vec<(String, SampleLoadingError)>,
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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    set_error_detail(true);

    let track_spec =
        unwrap_or_terminate(interpret_args(&args), ExitCodes::CantInterpretArgs.code());

    let beats = unwrap_or_terminate(load_samples(&track_spec), ExitCodes::CantLoadSamples.code());

    let beats_in_measure = beats[beats.len() - 1].time.floor() as u64;
    eprintln!("Measure has {beats_in_measure} beats");
    let track = Track {
        beats_per_minute: track_spec.beats_per_minute,
        beats_in_measure,
        measure_beats: beats,
    };

    let (sender, drumkick) = DrumkickBackend::new(
        DrumkickOpts {
            buffer_size: args.buffer_size,
            precision: args.precision,
            sound_capacity: 512,
            message_buffer_size: 32,
        },
        track,
    )?;

    drumkick.play()?;
    let mut playing = true;
    let mut bpm = track_spec.beats_per_minute;

    // poor and naive terminal reader, tmp.
    let mut reader = io::stdin().lock();
    loop {
        let mut buf = [0u8; 1];
        reader.read_exact(&mut buf)?;
        let character = buf[0] as char;
        match character {
            ' ' => {
                sender.send(if playing {
                    Message::Pause
                } else {
                    Message::Play
                })?;
                playing = !playing;
            }
            'e' | 'q' => {
                sender.send(Message::Stop)?;
                thread::sleep(time::Duration::from_millis(200));
                break;
            }
            '+' => {
                bpm += 5.0;
                sender.send(Message::SetBPM(bpm))?;
            }
            '-' => {
                bpm -= 5.0;
                sender.send(Message::SetBPM(bpm))?;
            }
            _ => {
                eprintln!("input {buf:?}");
            }
        }
    }

    Ok(())
}

/// Produce a map of sample names to sample paths
fn parse_sample_locations(samples: &[String]) -> Result<HashMap<&str, &str>, SampleLocationsError> {
    let split_opts = samples.iter().map(|s| s.split_once('='));
    let mut errors = Vec::new();
    let mut split = HashMap::new();
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
            beats_per_minute: args.bpm,
            measure_beats,
            ordered_samples: all_sample_names,
            sample_locations,
        }),
        (sample_locations, measure_beats) => Err(Box::new(ArgsParseError {
            sample_locations: sample_locations.err(),
            measure_parse_errors: measure_beats.err(),
        })),
    }
}

fn load_samples(track_spec: &TrackSpec) -> Result<Vec<Beat>, SampleLoadingErrors> {
    let sample_names: HashSet<_> = track_spec
        .measure_beats
        .iter()
        .flat_map(|bs| &bs.samples)
        .map(|s| s.as_str())
        .collect();
    let mut name_to_sample: HashMap<&str, StaticSoundData> = HashMap::new();
    let mut errors: Vec<(&str, SampleLoadingError)> = Vec::new();

    for name in sample_names {
        let sound_data = track_spec
            .sample_locations
            .get(name)
            .map(StaticSoundData::from_file)
            // Check for a sample embedded in the binary in case it isn't present in sample_locations.
            .or_else(|| get_embedded_sample(name).map(StaticSoundData::from_cursor));

        match sound_data {
            Some(Ok(sound_data)) => {
                eprintln!("Loaded sample {name} {}", sound_data.sample_rate);
                name_to_sample.insert(name, sound_data);
            }
            Some(Err(err)) => {
                errors.push((name, SampleLoadingError::FromFile(err)));
            }
            None => errors.push((
                name,
                SampleLoadingError::NoSuchSample(
                    track_spec
                        .ordered_samples
                        .iter()
                        .cloned()
                        .map(String::from)
                        .collect::<Vec<String>>(),
                ),
            )),
        }
    }

    if !errors.is_empty() {
        return Err(SampleLoadingErrors {
            errors: errors
                .into_iter()
                .map(|(s, err)| (s.to_string(), err))
                .collect(),
        });
    }

    Ok(track_spec
        .measure_beats
        .iter()
        .map(|bs| Beat {
            time: bs.time,
            samples: bs
                .samples
                .iter()
                .map(|sample_name| name_to_sample.get(sample_name.as_str()).unwrap().clone())
                .collect(),
        })
        .collect())
}
