use indexmap::{IndexMap, IndexSet};
use kira::{sound::FromFileError, sound::static_sound::StaticSoundData};
use std::collections::HashMap;
use thiserror::Error;

use crate::{
    Beat,
    embedded_samples::{get_embedded_sample, get_embedded_sample_names},
    measure_grammar::{BeatSpec, backtick_quote, join_backtick_quoted},
};

#[derive(Error, Debug)]
#[error(
    "no such sample{}: {} - available sample names are: {}",
    if .missing_samples.len() > 1 {"s"} else {""},
    join_backtick_quoted(.missing_samples.iter().map(String::as_ref)),
    join_backtick_quoted(.available_samples.iter().map(String::as_ref)),
)]
pub struct MissingSamples {
    missing_samples: Vec<String>,
    available_samples: Vec<String>,
}

#[derive(Error, Debug)]
#[error(
    "{} error{} loading samples:{}{}{}{}",
    self.error_count(),
    if self.has_multiple_errors() { "s" } else { "" },
    if self.has_multiple_errors() { "\n" } else { " " },
    .missing_samples.as_ref().map_or_default(ToString::to_string),
    if .missing_samples.is_some() { "\n" } else { "" },
    .from_file_errors.iter().map(|(sample, err)| format!("error loading sample {}: {err}", backtick_quote(sample))).collect::<Vec<_>>().join("\n")
)]
pub struct SampleLoadingErrors {
    missing_samples: Option<MissingSamples>,
    from_file_errors: Vec<(String, FromFileError)>,
}

impl SampleLoadingErrors {
    fn error_count(&self) -> usize {
        self.from_file_errors.len() + self.missing_samples.is_some() as usize
    }

    fn has_multiple_errors(&self) -> bool {
        self.error_count() > 1
    }
}

pub fn load_samples<'a>(
    measure_beats: &[BeatSpec],
    sample_locations: &IndexMap<&'a str, &'a str>,
) -> Result<Vec<Beat>, SampleLoadingErrors> {
    let mut name_to_sample: HashMap<&str, StaticSoundData> = HashMap::new();
    let mut missing_samples: Vec<String> = Vec::new();
    let mut errors: Vec<(&str, FromFileError)> = Vec::new();

    let unique_beat_sample_names: IndexSet<_> = measure_beats
        .iter()
        .flat_map(|bs| &bs.samples)
        .map(|s| s.as_str())
        .collect();

    for name in unique_beat_sample_names {
        let sound_data = sample_locations
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
                errors.push((name, err));
            }
            None => {
                missing_samples.push(name.to_owned());
            }
        }
    }

    let missing_samples_error = if !missing_samples.is_empty() {
        let all_sample_names: IndexSet<_> = get_embedded_sample_names()
            .into_iter()
            .chain(sample_locations.keys().cloned())
            .collect();

        Some(MissingSamples {
            missing_samples,
            available_samples: all_sample_names
                .iter()
                .cloned()
                .map(String::from)
                .collect::<Vec<String>>(),
        })
    } else {
        None
    };

    if !errors.is_empty() || missing_samples_error.is_some() {
        return Err(SampleLoadingErrors {
            missing_samples: missing_samples_error,
            from_file_errors: errors
                .into_iter()
                .map(|(s, err)| (s.to_string(), err))
                .collect(),
        });
    }

    Ok(measure_beats
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
