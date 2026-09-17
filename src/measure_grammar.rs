use pest::error::Error as PestError;
use pest::iterators::Pair;
use pest::Parser as PestParser;
use pest_derive::Parser as PestParser;
use std::collections::HashMap;
use std::sync::LazyLock;
use thiserror::Error;

#[derive(Debug, PestParser)]
#[grammar = "measure.pest"]
struct MeasureGrammar;

#[derive(Debug, PartialEq)]
pub struct BeatSpec {
    pub samples: Vec<String>,
    pub time: f64,
}

#[derive(Error, Debug, PartialEq)]
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
    let mut position_str = "1";
    let mut position = 1.0;

    for measure_component in measure_ast.into_inner() {
        match measure_component.as_rule() {
            Rule::beat => {
                let mut samples = Vec::new();

                for beat_component in measure_component.into_inner() {
                    let mut new_position_str = position_str;
                    let mut new_position = position;

                    match beat_component.as_rule() {
                        Rule::placement => {
                            let placement =
                                beat_component.into_inner().next().expect("beat placement");

                            new_position_str = placement.as_str();
                            new_position = match placement.as_rule() {
                                Rule::beat_no => new_position_str
                                    .parse::<u64>()
                                    .expect("beat placement must be numeric")
                                    as f64,
                                Rule::beat_fraction_name => {
                                    position.floor() + name_to_beat_fraction(new_position_str)
                                }
                                Rule::beat_fraction => {
                                    let (numerator_str, denominator_str) =
                                        new_position_str.split_once('/').expect("fraction string");
                                    let numerator: f64 =
                                        numerator_str.parse().expect("numerator f64");
                                    let denominator: f64 =
                                        denominator_str.parse().expect("denominator f64");
                                    position.floor() + numerator / denominator
                                }
                                _ => unreachable!("in beat placement {}", new_position_str),
                            }
                        }
                        Rule::samples_list => {
                            samples.extend(
                                beat_component
                                    .into_inner()
                                    .map(|sample| sample.as_str().to_owned()),
                            );
                        }
                        Rule::EOI => (),
                        other_rule => {
                            unreachable!(
                                "rule {:?} in beat component {}",
                                other_rule,
                                beat_component.as_str()
                            )
                        }
                    };

                    if new_position < position {
                        return Err(format!(
                            "beat number decreased: {} → {}",
                            backtick_quote(position_str),
                            backtick_quote(new_position_str)
                        ));
                    }

                    position = new_position;
                    position_str = new_position_str;
                }

                beats.push(BeatSpec {
                    samples,
                    time: position,
                });
            }
            Rule::EOI => (),
            other_rule => {
                unreachable!(
                    "rule {:?} in measure component {}",
                    other_rule,
                    measure_component.as_str()
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

fn rename_rule(rule: &Rule, all_sample_names: &[&str]) -> String {
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
        Rule::non_zero_digit => "non-zero digit".to_string(),
        Rule::beat_fraction => "beat fraction (e.g. `1/3`)".to_string(),
        Rule::placement => "placement (e.g. `1`, `and`, `1/3`)".to_string(),
        Rule::beat => "beat (e.g. `1:kick,hat`)".to_string(),
        Rule::measure => "measure (e.g. `1:kick 2:kick,snare, ...``".to_string(),
        Rule::EOI => "end of input".to_string(),
    }
}

pub(crate) fn parse_measure(
    measure: &str,
    all_sample_names: &[&str],
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

#[cfg(test)]
mod test {
    use super::*;
    use rstest::*;
    use std::assert_matches;
    const STANDARD_SAMPLE_NAMES: &[&str] = &["CLICK", "click", "DRIP", "drip"];

    #[rstest]
    fn test_parse_measure_simple(
        #[values(
            "1:click",
            " 1:click",
            "  1:click",
            "1:click ",
            "1:click  ",
            "  1:click  "
        )]
        measure_str: &str,
    ) {
        assert_eq!(
            parse_measure(measure_str, STANDARD_SAMPLE_NAMES).unwrap(),
            [BeatSpec {
                samples: vec!["click".to_string()],
                time: 1.0,
            }]
        );
    }

    #[rstest]
    fn test_parse_measure_beats() {
        assert_eq!(
            parse_measure("1:CLICK 2:click 3:click", STANDARD_SAMPLE_NAMES).unwrap(),
            [
                BeatSpec {
                    samples: vec!["CLICK".to_string()],
                    time: 1.0,
                },
                BeatSpec {
                    samples: vec!["click".to_string()],
                    time: 2.0,
                },
                BeatSpec {
                    samples: vec!["click".to_string()],
                    time: 3.0,
                },
            ]
        );
    }

    #[rstest]
    fn test_parse_measure_beats_without_samples_at_beginning_and_end() {
        assert_eq!(
            parse_measure("1: 2:click 3:", STANDARD_SAMPLE_NAMES).unwrap(),
            [
                BeatSpec {
                    samples: vec![],
                    time: 1.0,
                },
                BeatSpec {
                    samples: vec!["click".to_string()],
                    time: 2.0,
                },
                BeatSpec {
                    samples: vec![],
                    time: 3.0,
                },
            ]
        );
    }

    #[rstest]
    fn test_parse_measure_beats_without_samples_in_between_other_beats() {
        assert_eq!(
            parse_measure("1:CLICK 2: 3:click", STANDARD_SAMPLE_NAMES).unwrap(),
            [
                BeatSpec {
                    samples: vec!["CLICK".to_string()],
                    time: 1.0,
                },
                BeatSpec {
                    samples: vec![],
                    time: 2.0,
                },
                BeatSpec {
                    samples: vec!["click".to_string()],
                    time: 3.0,
                },
            ]
        );
    }

    #[rstest]
    fn test_parse_measure_ands() {
        assert_eq!(
            parse_measure("1:DRIP and:click 2:CLICK &:click", STANDARD_SAMPLE_NAMES).unwrap(),
            [
                BeatSpec {
                    samples: vec!["DRIP".to_string()],
                    time: 1.0,
                },
                BeatSpec {
                    samples: vec!["click".to_string()],
                    time: 1.5,
                },
                BeatSpec {
                    samples: vec!["CLICK".to_string()],
                    time: 2.0,
                },
                BeatSpec {
                    samples: vec!["click".to_string()],
                    time: 2.5,
                },
            ]
        );
    }

    #[rstest]
    fn test_parse_measure_e_and_ah(
        #[values(
            "1:DRIP e:hat and:click a:hat 2:CLICK",
            "1:DRIP e:hat &:click ah:hat 2:CLICK",
            "1:DRIP e:hat &:click uh:hat 2:CLICK"
        )]
        measure_str: &str,
    ) {
        assert_eq!(
            parse_measure(measure_str, STANDARD_SAMPLE_NAMES).unwrap(),
            [
                BeatSpec {
                    samples: vec!["DRIP".to_string()],
                    time: 1.0,
                },
                BeatSpec {
                    samples: vec!["hat".to_string()],
                    time: 1.25,
                },
                BeatSpec {
                    samples: vec!["click".to_string()],
                    time: 1.5,
                },
                BeatSpec {
                    samples: vec!["hat".to_string()],
                    time: 1.75,
                },
                BeatSpec {
                    samples: vec!["CLICK".to_string()],
                    time: 2.0,
                },
            ]
        );
    }

    #[rstest]
    fn test_parse_measure_triplets(
        #[values(
            "1:DRIP tri:hat plet:click 2:CLICK",
            "1:DRIP trip:hat let:click 2:CLICK",
            "1:DRIP trip:hat plet:click 2:CLICK"
        )]
        measure_str: &str,
    ) {
        assert_eq!(
            parse_measure(measure_str, STANDARD_SAMPLE_NAMES).unwrap(),
            [
                BeatSpec {
                    samples: vec!["DRIP".to_string()],
                    time: 1.0,
                },
                BeatSpec {
                    samples: vec!["hat".to_string()],
                    time: 1.0 + 1.0 / 3.0,
                },
                BeatSpec {
                    samples: vec!["click".to_string()],
                    time: 1.0 + 2.0 / 3.0,
                },
                BeatSpec {
                    samples: vec!["CLICK".to_string()],
                    time: 2.0,
                },
            ]
        );
    }

    #[rstest]
    fn test_parse_measure_fractions() {
        assert_eq!(
            parse_measure(
                "1:CLICK 1/5:click 2/5:click 3/5:click 4/5:click 2:CLICK",
                STANDARD_SAMPLE_NAMES
            )
            .unwrap(),
            [
                BeatSpec {
                    samples: vec!["CLICK".to_string()],
                    time: 1.0,
                },
                BeatSpec {
                    samples: vec!["click".to_string()],
                    time: 1.0 + 1.0 / 5.0,
                },
                BeatSpec {
                    samples: vec!["click".to_string()],
                    time: 1.0 + 2.0 / 5.0,
                },
                BeatSpec {
                    samples: vec!["click".to_string()],
                    time: 1.0 + 3.0 / 5.0,
                },
                BeatSpec {
                    samples: vec!["click".to_string()],
                    time: 1.0 + 4.0 / 5.0,
                },
                BeatSpec {
                    samples: vec!["CLICK".to_string()],
                    time: 2.0,
                },
            ]
        );
    }

    #[rstest]
    fn test_parse_measure_multiple_samples() {
        assert_eq!(
            parse_measure("1:click,drip", STANDARD_SAMPLE_NAMES).unwrap(),
            [BeatSpec {
                samples: vec!["click".to_string(), "drip".to_string()],
                time: 1.0,
            }]
        );

        assert_eq!(
            parse_measure(
                "1:DRIP trip:hat,click plet:click,hat 2:CLICK",
                STANDARD_SAMPLE_NAMES
            )
            .unwrap(),
            [
                BeatSpec {
                    samples: vec!["DRIP".to_string()],
                    time: 1.0,
                },
                BeatSpec {
                    samples: vec!["hat".to_string(), "click".to_string()],
                    time: 1.0 + 1.0 / 3.0,
                },
                BeatSpec {
                    samples: vec!["click".to_string(), "hat".to_string()],
                    time: 1.0 + 2.0 / 3.0,
                },
                BeatSpec {
                    samples: vec!["CLICK".to_string()],
                    time: 2.0,
                },
            ]
        );
    }

    #[rstest]
    #[case("1:click 2:click 1:click", "beat number decreased: `2` → `1`")]
    #[case("1:click and:click e:click", "beat number decreased: `and` → `e`")]
    #[case("1:click 2/3:click 1/3:click ", "beat number decreased: `2/3` → `1/3`")]
    fn test_parse_measure_error_beat_number_decreased(
        #[case] measure_str: &str,
        #[case] error: &str,
    ) {
        assert_eq!(
            parse_measure(measure_str, STANDARD_SAMPLE_NAMES),
            Err(MeasureParseError::LogicalError(error.into())),
        );
    }

    #[rstest]
    #[case::no_beats(
        "",
        "
1 | 
  | ^---
  |
  = expected placement (e.g. `1`, `and`, `1/3`)"
    )]
    #[case::beat_1_space(
        "1: click",
        "
1 | 1: click
  |    ^---
"
    )]
    #[case::beat_1_no_colon(
        "1click",
        "
1 | 1click
  | ^---
  |
  = expected beat (e.g. `1:kick,hat`)"
    )]
    #[case::bad_fraction_1_over_0(
        "1:click 1/0:CLICK",
        "
1 | 1:click 1/0:CLICK
  |           ^---
"
    )]
    #[case::bad_fraction_0_over_0(
        "1:click 0/0:CLICK",
        "
1 | 1:click 0/0:CLICK
  |         ^---
"
    )]
    #[case::bad_fraction_0_over_0(
        "1:click 0/0:CLICK",
        "
1 | 1:click 0/0:CLICK
  |         ^---
"
    )]
    fn test_parse_measure_error_beat_syntax_errors(
        #[case] measure_str: &str,
        #[case] expected_error: &str,
    ) {
        let err = parse_measure(measure_str, STANDARD_SAMPLE_NAMES);
        assert_matches!(err, Err(MeasureParseError::SyntaxError(_)));

        let syntax_error = err.err().unwrap().to_string();
        assert!(
            syntax_error.contains(expected_error),
            "EXPECTED SUBSTRING:\n{}\nIN:\n{}",
            expected_error,
            syntax_error
        );
    }
}
