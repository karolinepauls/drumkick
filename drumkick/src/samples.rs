use indexmap::{IndexMap, IndexSet};
use std::collections::HashMap;
use std::sync::Arc;
use symphonia::core::errors::Error as SymphoniaError;
use thiserror::Error;

use crate::{
    backend::machine::{Sequence, SequenceItem},
    backend::sound,
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
    from_file_errors: Vec<(String, SymphoniaError)>,
}

impl SampleLoadingErrors {
    fn error_count(&self) -> usize {
        self.from_file_errors.len() + self.missing_samples.is_some() as usize
    }

    fn has_multiple_errors(&self) -> bool {
        self.error_count() > 1
    }
}

pub fn load_samples_into_sequence<'a>(
    measure_beats: &[BeatSpec],
    sample_locations: &IndexMap<&'a str, &'a str>,
) -> Result<Sequence, SampleLoadingErrors> {
    let mut name_to_sample: HashMap<&str, Arc<sound::LoadedSource>> = HashMap::new();
    let mut missing_samples: Vec<String> = Vec::new();
    let mut errors: Vec<(&str, SymphoniaError)> = Vec::new();

    let unique_beat_sample_names: IndexSet<_> = measure_beats
        .iter()
        .flat_map(|bs| &bs.samples)
        .map(|s| s.as_str())
        .collect();

    for name in unique_beat_sample_names {
        let sound_data = sample_locations
            .get(name)
            .map(|path| sound::load_from_path(path))
            // Check for a sample embedded in the binary in case it isn't present in sample_locations.
            .or_else(|| get_embedded_sample(name).map(sound::load_from_cursor));

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

    let items = measure_beats.iter().flat_map(|bs| {
        let samples = bs
            .samples
            .iter()
            .map(|sample_name| name_to_sample.get(sample_name.as_str()).unwrap().clone());
        samples.map(|sample| SequenceItem::new(bs.beat_no, sample))
    });

    Ok(Sequence::new(items))
}

#[cfg(test)]
mod test {
    use super::*;
    use rstest::*;

    use crate::sequence::BeatNo;

    #[rstest]
    fn test_load_embedded_samples() {
        let beats = vec![
            BeatSpec {
                samples: vec![String::from("CLICK"), String::from("click")],
                beat_no: BeatNo::ONE,
            },
            BeatSpec {
                samples: vec![String::from("DRIP"), String::from("drip")],
                beat_no: BeatNo::even(2).unwrap(),
            },
        ];
        let sequence = load_samples_into_sequence(&beats, &IndexMap::new()).unwrap();

        assert_eq!(sequence.beat_count(), 2);
        assert_eq!(sequence.items.len(), 4);
    }
}
