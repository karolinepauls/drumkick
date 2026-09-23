use kira::{sound::static_sound::StaticSoundData, sound::FromFileError};
use std::collections::{HashMap, HashSet};
use thiserror::Error;

use crate::{
    embedded_samples::get_embedded_sample,
    measure_grammar::{join_backtick_quoted, BeatSpec},
    Beat,
};

#[derive(Error, Debug)]
pub enum SampleLoadingError {
    #[error("no such sample - available sample names are: {}", join_backtick_quoted(.0.iter().map(String::as_ref)))]
    NoSuchSample(Vec<String>),
    #[error("sample loading failed {0}")]
    FromFile(FromFileError),
}

#[derive(Error, Debug)]
#[error(
    "error{} loading samples:{}{}",
    if .errors.len() > 1 {"s"} else {""},
    if .errors.len() > 1 {"\n"} else {" "},
    .errors.iter().map(|(sample, err)| format!("{:?}: {}", sample, err)).collect::<Vec<_>>().join("\n")
)]
pub struct SampleLoadingErrors {
    errors: Vec<(String, SampleLoadingError)>,
}

pub fn load_samples<'a>(
    measure_beats: &[BeatSpec],
    ordered_samples: &[&'a str],
    sample_locations: &HashMap<&'a str, &'a str>,
) -> Result<Vec<Beat>, SampleLoadingErrors> {
    let used_sample_names: HashSet<_> = measure_beats
        .iter()
        .flat_map(|bs| &bs.samples)
        .map(|s| s.as_str())
        .collect();

    let mut name_to_sample: HashMap<&str, StaticSoundData> = HashMap::new();
    let mut errors: Vec<(&str, SampleLoadingError)> = Vec::new();
    for name in used_sample_names {
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
                errors.push((name, SampleLoadingError::FromFile(err)));
            }
            None => {
                let available_sample_names = ordered_samples
                    .iter()
                    .cloned()
                    .map(String::from)
                    .collect::<Vec<String>>();

                errors.push((
                    name,
                    SampleLoadingError::NoSuchSample(available_sample_names),
                ));
            }
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
