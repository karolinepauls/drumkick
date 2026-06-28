use clap::{Parser, builder::RangedU64ValueParser, value_parser};
use kira::backend::cpal::CpalBackendSettings;
use kira::backend::cpal::cpal::{
    BufferSize, default_host,
    traits::{DeviceTrait, HostTrait},
};
use kira::{
    AudioManager, AudioManagerSettings, DefaultBackend,
    clock::{ClockSpeed, ClockTime},
    sound::FromFileError,
    sound::static_sound::{StaticSoundData, StaticSoundHandle, StaticSoundSettings},
};
use pest::set_error_detail;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::{thread, time};
use thiserror::Error;

mod measure_grammar;
use measure_grammar::*;

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
#[command(version, about, long_about = None)]
struct Args {
    #[arg(default_value_t = "1:hat,kick e:hat and:hat ah:hat 2:hat,snare 3:hat,kick 4:hat,snare".to_string())]
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
        default_value_t = 32,
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
        default_value_t = 2048,
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
    sample_locations: HashMap<&'a str, &'a str>,
}

#[derive(Debug)]
struct Beat {
    samples: Vec<StaticSoundData>,
    time: f64,
}

#[derive(Debug)]
struct Track {
    beats_per_minute: f64,
    beats_in_measure: u64,
    measure_beats: Vec<Beat>,
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
    #[error("no such sample")]
    NoSuchSample,
    #[error("samples loading failed {0}")]
    FromFile(FromFileError),
}

const MIN_MEASURES: u64 = 4;

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
            eprintln!("{}", e);
            std::process::exit(error_code);
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    set_error_detail(true);
    let track_spec =
        unwrap_or_terminate(interpret_args(&args), ExitCodes::CantInterpretArgs.code());
    let beats = unwrap_or_terminate(
        load_samples(&track_spec.measure_beats, &track_spec.sample_locations),
        ExitCodes::CantLoadSamples.code(),
    );
    let beats_in_measure = beats[beats.len() - 1].time.floor() as u64;
    eprintln!("Measure has {} beats", beats_in_measure);
    let track = Track {
        beats_per_minute: track_spec.beats_per_minute,
        beats_in_measure,
        measure_beats: beats,
    };

    let host = default_host();
    let device = host.default_output_device().unwrap();
    let mut cpal_config = device.default_output_config()?.config();
    cpal_config.buffer_size = BufferSize::Fixed(args.buffer_size);

    let cpal_settings = CpalBackendSettings {
        device: Some(device),
        config: Some(cpal_config),
    };
    let settings = AudioManagerSettings {
        backend_settings: cpal_settings,
        internal_buffer_size: args.precision,
        ..AudioManagerSettings::default()
    };
    let mut manager = AudioManager::<DefaultBackend>::new(settings)?;
    let mut clock = manager.add_clock(ClockSpeed::TicksPerMinute(track.beats_per_minute))?;

    let mut next_beat_at = clock.time();

    let beats_per_second = track.beats_per_minute / 60.0;
    let seconds_per_measure = track.beats_in_measure as f64 / beats_per_second;
    let measures_ahead_float = (beats_per_second * 2.0).ceil();
    assert!(
        measures_ahead_float.is_finite(),
        "Expeting measures ahead to be finite but it's {}",
        measures_ahead_float
    );
    let measures_ahead = (measures_ahead_float as u64).max(MIN_MEASURES);
    eprintln!("Maintaining measures ahead: {}", measures_ahead);

    let (_, next_beat_at_) = schedule_beats(
        &mut manager,
        next_beat_at,
        &track.measure_beats,
        track.beats_in_measure,
        measures_ahead,
    )?;
    next_beat_at = next_beat_at_;

    clock.start();
    eprintln!("Metronome started");

    loop {
        if clock.time() + measures_ahead / 2 > next_beat_at {
            eprintln!("Scheduled {} measures", measures_ahead);

            let (_, new_next_beat_at) = schedule_beats(
                &mut manager,
                next_beat_at,
                &track.measure_beats,
                track.beats_in_measure,
                measures_ahead,
            )?;
            next_beat_at = new_next_beat_at;
        }
        thread::sleep(time::Duration::from_secs_f64(seconds_per_measure));
    }
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

    let all_sample_names: Vec<&str> = sample_locations
        .as_ref()
        .map_or_else(|_e| Vec::new(), |locs| locs.keys().copied().collect());

    let measure_beats = parse_measure(&args.measure, &all_sample_names);

    match (sample_locations, measure_beats) {
        (Ok(sample_locations), Ok(measure_beats)) => Ok(TrackSpec {
            beats_per_minute: args.bpm,
            measure_beats,
            sample_locations,
        }),
        (sample_locations, measure_beats) => Err(Box::new(ArgsParseError {
            sample_locations: sample_locations.err(),
            measure_parse_errors: measure_beats.err(),
        })),
    }
}

fn load_samples(
    beat_specs: &[BeatSpec],
    sample_locations: &HashMap<&str, &str>,
) -> Result<Vec<Beat>, SampleLoadingErrors> {
    let sample_names: HashSet<_> = beat_specs
        .iter()
        .flat_map(|bs| &bs.samples)
        .map(|s| s.as_str())
        .collect();
    let mut samples: HashMap<&str, StaticSoundData> = HashMap::new();
    let mut errors: Vec<(&str, SampleLoadingError)> = Vec::new();

    for name in sample_names {
        if let Some(location) = sample_locations.get(name) {
            match StaticSoundData::from_file(location) {
                Ok(sound_data) => {
                    eprintln!("Loaded sample {} {}", name, sound_data.sample_rate);
                    samples.insert(name, sound_data);
                }
                Err(err) => {
                    errors.push((name, SampleLoadingError::FromFile(err)));
                }
            }
        } else {
            errors.push((name, SampleLoadingError::NoSuchSample));
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

    Ok(beat_specs
        .iter()
        .map(|bs| Beat {
            time: bs.time,
            samples: bs
                .samples
                .iter()
                .map(|sample_name| samples.get(sample_name.as_str()).unwrap().clone())
                .collect(),
        })
        .collect())
}

fn schedule_beats(
    manager: &mut AudioManager,
    start_at: ClockTime,
    beats: &[Beat],
    beats_in_measure: u64,
    measures: u64,
) -> Result<(Vec<StaticSoundHandle>, ClockTime), Box<dyn std::error::Error>> {
    let mut sounds = vec![];

    for measure_no in 0..measures {
        let measure_time = start_at + beats_in_measure * measure_no;

        for beat in beats.iter() {
            let beat_time = measure_time + beat.time;

            for sample in &beat.samples {
                sounds.push(
                    manager.play(
                        sample
                            .clone()
                            .with_settings(StaticSoundSettings::new().start_time(beat_time)),
                    )?,
                );
            }
        }
    }

    Ok((sounds, start_at + beats_in_measure * measures))
}
