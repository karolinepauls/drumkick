use pest::Parser as PestParser;
use pest::error::Error as PestError;
use pest::iterators::Pair;
use pest_derive::Parser as PestParser;
use std::collections::HashMap;
use std::sync::LazyLock;
use thiserror::Error;

#[derive(Debug, PestParser)]
#[grammar = "measure.pest"]
struct MeasureGrammar;

#[derive(Debug)]
pub struct BeatSpec {
    pub samples: Vec<String>,
    pub time: f64,
}

#[derive(Error, Debug)]
pub enum MeasureParseError {
    #[error("{0}")]
    SyntaxError(PestError<Rule>),
    #[error("logical error: {0}")]
    LogicalError(String),
}

pub(crate) static BEAT_FRACTIONS: &[(&str, f64)] = &[
    ("e", 0.25),
    ("&", 0.5),
    ("and", 0.5),
    ("ah", 0.75),
    ("uh", 0.75),
    ("a", 0.75),
    ("trip", 1.0 / 3.0),
    ("tri", 1.0 / 3.0),
    ("let", 2.0 / 3.0),
    ("plet", 2.0 / 3.0),
];

static BEAT_FRACTION_MAP: LazyLock<HashMap<&str, f64>> =
    LazyLock::new(|| BEAT_FRACTIONS.iter().copied().collect());

fn name_to_beat_fraction(name: &str) -> f64 {
    *(BEAT_FRACTION_MAP
        .get(name)
        .unwrap_or_else(|| panic!("beat fraction defined for name: {}", name)))
}

fn parse_measure_ast(measure_ast: Pair<Rule>) -> Result<Vec<BeatSpec>, String> {
    let mut beats = Vec::new();
    let mut position = 1.0;

    for beat in measure_ast.into_inner() {
        match beat.as_rule() {
            Rule::beat => {
                for beat in beat.into_inner() {
                    let mut new_position = position;
                    match beat.as_rule() {
                        Rule::placement => {
                            let placement = beat.into_inner().next().expect("beat placement");
                            match placement.as_rule() {
                                Rule::beat_no => {
                                    new_position = placement
                                        .as_str()
                                        .parse::<u64>()
                                        .expect("beat placement must be numeric")
                                        as f64;
                                }
                                Rule::beat_fraction_name => {
                                    new_position = position.floor()
                                        + name_to_beat_fraction(placement.as_str());
                                }
                                Rule::beat_fraction => {
                                    new_position = position.floor()
                                        + placement.as_str().parse::<f64>().unwrap();
                                }
                                _ => unreachable!("in placement {}", placement.as_str()),
                            }
                        }
                        Rule::samples_list => {
                            let samples = beat
                                .into_inner()
                                .map(|sample| sample.as_str().to_owned())
                                .collect();

                            beats.push(BeatSpec {
                                samples,
                                time: position,
                            });
                        }
                        Rule::EOI => (),
                        other_rule => {
                            unreachable!(
                                "rule {:?} in beat component {}",
                                other_rule,
                                beat.as_str()
                            )
                        }
                    }
                    if new_position < position {
                        return Err(format!(
                            "beat number decreased: {} → {}",
                            position, new_position
                        ));
                    }
                    position = new_position;
                }
            }
            Rule::EOI => (),
            other_rule => {
                unreachable!(
                    "rule {:?} in measure component {}",
                    other_rule,
                    beat.as_str()
                )
            }
        }
    }

    Ok(beats)
}

pub(crate) fn backtick_quote(k: &str) -> String {
    format!("`{}`", k.replace("`", "\\`"))
}

pub(crate) fn join_backtick_quoted<'a>(items: impl Iterator<Item = &'a str>) -> String {
    items.map(backtick_quote).collect::<Vec<_>>().join(", ")
}

fn rename_rule(rule: &Rule, all_sample_names: &Vec<&str>) -> String {
    match rule {
        Rule::sample_name => {
            let all_sample_names = join_backtick_quoted(all_sample_names.iter().cloned());
            format!("sample name (one of: {})", all_sample_names)
        }
        Rule::samples_list => "comma-separated list of samples (e.g. `kick,hat`)".to_string(),
        Rule::beat_no => "beat no (`1`, `2`, etc)".to_string(),
        Rule::beat_fraction_name => {
            let all_fraction_names =
                join_backtick_quoted(BEAT_FRACTIONS.iter().map(|(k, _)| k).cloned());
            format!("beat fraction name (one of: {})", all_fraction_names)
        }
        Rule::beat_fraction => "beat fraction (e.g. `1/3`)".to_string(),
        Rule::placement => "placement (e.g. `1`, `and`, `1/3`)".to_string(),
        Rule::beat => "beat (e.g. `1:kick,hat`)".to_string(),
        Rule::measure => "measure (e.g. `1:kick 2:kick,snare, ...``".to_string(),
        Rule::EOI => "end of input".to_string(),
    }
}

pub(crate) fn parse_measure(
    measure: &str,
    all_sample_names: &Vec<&str>,
) -> Result<Vec<BeatSpec>, MeasureParseError> {
    MeasureGrammar::parse(Rule::measure, measure)
        .map_err(|e| {
            MeasureParseError::SyntaxError(
                e.renamed_rules(|rule| rename_rule(rule, all_sample_names)),
            )
        })
        .and_then(|mut grammar| {
            parse_measure_ast(grammar.next().unwrap()).map_err(MeasureParseError::LogicalError)
        })
}
