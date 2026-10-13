use indexmap::IndexMap;
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
    "{} error{} loading samples:{}{}",
    self.error_count(),
    if self.has_multiple_errors() { "s" } else { "" },
    if self.has_multiple_errors() { "\n" } else { " " },
    .errors.iter().map(|(sample, err)| format!("error loading sample {}: {err}", backtick_quote(sample))).collect::<Vec<_>>().join("\n")
)]
pub struct SampleLoadingErrors {
    errors: Vec<(String, SymphoniaError)>,
}

impl SampleLoadingErrors {
    fn error_count(&self) -> usize {
        self.errors.len()
    }

    fn has_multiple_errors(&self) -> bool {
        self.error_count() > 1
    }
}

fn load_embedded_sample(name: &str) -> Result<Option<Arc<sound::LoadedSource>>, SymphoniaError> {
    get_embedded_sample(name)
        .map(sound::load_from_cursor)
        .transpose()
}

pub fn load_samples<'a>(
    sample_locations: &IndexMap<&str, &str>,
) -> Result<IndexMap<String, Arc<sound::LoadedSource>>, SampleLoadingErrors> {
    let mut name_to_sample: IndexMap<String, Arc<sound::LoadedSource>> = IndexMap::new();
    let mut errors: Vec<(&str, SymphoniaError)> = Vec::new();

    for (&name, &path) in sample_locations.iter() {
        let sound_data = sound::load_from_path(path);

        match sound_data {
            Ok(sound_data) => {
                eprintln!("Loaded sample {name} {}", sound_data.sample_rate);
                name_to_sample.insert(name.to_string(), sound_data);
            }
            Err(err) => {
                errors.push((name, err));
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

    Ok(name_to_sample)
}

pub fn load_samples_into_sequence(
    beats: &[BeatSpec],
    name_to_sample: &IndexMap<String, Arc<sound::LoadedSource>>,
) -> Result<Sequence, MissingSamples> {
    let mut items = Vec::new();
    let mut missing_samples = Vec::new();

    for beat in beats {
        for sample_name in beat.samples.iter() {
            let loaded_sample = name_to_sample
                .get(sample_name.as_str())
                .map(Arc::clone)
                // Check for a sample embedded in the binary in case it isn't present in sample_locations.
                .or_else(|| {
                    load_embedded_sample(sample_name.as_str()).unwrap_or_else(|e| {
                        panic!("loading the embedded sample {sample_name} should not fail: {e}")
                    })
                });
            match loaded_sample {
                Some(sample) => items.push(SequenceItem::new(beat.beat_no, sample.clone())),
                None => missing_samples.push(sample_name.to_string()),
            }
        }
    }

    if !missing_samples.is_empty() {
        return Err(MissingSamples {
            missing_samples,
            available_samples: name_to_sample
                .keys()
                .cloned()
                .chain(get_embedded_sample_names().into_iter().map(String::from))
                .collect(),
        });
    }

    Ok(Sequence::new(items))
}

#[cfg(test)]
mod test {
    use super::*;
    use indexmap::IndexSet;
    use rstest::*;

    use crate::sequence::BeatNo;

    fn sound_samples_to_bytes(samples: &Vec<f32>) -> Vec<u8> {
        samples
            .iter()
            .flat_map(|&s| s.to_ne_bytes())
            .collect::<Vec<_>>()
    }

    #[rstest]
    fn test_load_embedded_samples() {
        let parent_dir = env!("CARGO_MANIFEST_DIR");
        let files = [
            "click_strong.mp3",
            "click_weak.mp3",
            "drip_strong.mp3",
            "drip_weak.mp3",
        ];
        let beats_vec: Vec<(&str, String)> = files
            .iter()
            .map(|&s| {
                let mut path = parent_dir.to_string();
                path.push_str("/samples/");
                path.push_str(s);
                (s, path)
            })
            .collect();
        let beats: IndexMap<&str, &str> = beats_vec.iter().map(|(s, p)| (*s, p.as_str())).collect();

        let samples = load_samples(&beats).unwrap();

        assert_eq!(samples.len(), 4);
        assert_eq!(
            samples.keys().collect::<Vec<_>>(),
            vec![
                "click_strong.mp3",
                "click_weak.mp3",
                "drip_strong.mp3",
                "drip_weak.mp3",
            ]
        );
        let loaded_sources: IndexSet<Vec<u8>> = samples
            .values()
            .map(|loaded| sound_samples_to_bytes(&loaded.samples))
            .collect();
        assert_eq!(loaded_sources.len(), 4);
    }

    #[rstest]
    fn test_load_embedded_samples_into_sequence() {
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
        assert_eq!(sequence.items.len(), 4);
        assert_eq!(sequence.beat_count(), 2);

        assert_eq!(
            sequence
                .items
                .iter()
                .map(|item| item.beat_no)
                .collect::<Vec<_>>(),
            vec![
                BeatNo::ONE,
                BeatNo::ONE,
                BeatNo::even(2).unwrap(),
                BeatNo::even(2).unwrap(),
            ]
        );

        assert_eq!(
            sequence
                .items
                .iter()
                .map(|item| item.beat_no)
                .collect::<Vec<_>>(),
            vec![
                BeatNo::ONE,
                BeatNo::ONE,
                BeatNo::even(2).unwrap(),
                BeatNo::even(2).unwrap(),
            ]
        );

        let loaded_sources: IndexSet<Vec<u8>> = sequence
            .items
            .iter()
            .map(|item| sound_samples_to_bytes(&item.source.samples))
            .collect();
        assert_eq!(loaded_sources.len(), 4);
    }
}
