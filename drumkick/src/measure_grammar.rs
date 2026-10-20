use crate::beat::{BeatFraction, BeatNo};
use pest::Parser as PestParser;
use pest::error::Error as PestError;
use pest::iterators::Pair;
use pest_derive::Parser as PestParser;
use std::collections::HashMap;
use std::fmt;
use std::num::NonZeroU32;
use std::sync::LazyLock;
use thiserror::Error;

#[derive(Debug, PestParser)]
#[grammar = "measure.pest"]
struct MeasureGrammar;

pub trait ToSource {
    fn to_source(&self) -> String;
}

#[derive(Debug, Clone, PartialEq)]
pub struct BeatSpec {
    pub samples: Vec<String>,
    pub beat_no: BeatNo,
}

impl fmt::Display for BeatSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        format!("{}:{}", self.beat_no, self.samples.join(",")).fmt(f)
    }
}

impl ToSource for BeatSpec {
    fn to_source(&self) -> String {
        self.to_string()
    }
}

impl ToSource for [BeatSpec] {
    /// Produce canonical syntax useful for pretty-printing.
    fn to_source(&self) -> String {
        let mut source = String::with_capacity(64);

        let mut last_beat_no_integer = 0;
        for beat_spec in self.iter() {
            // If moving from 1 to e.g. 2 1/2, then make it "... 2: 1/2:..."
            if beat_spec.beat_no.beat.get() != last_beat_no_integer {
                source.push_str(&format!("{}:", beat_spec.beat_no.beat));
            }
            if beat_spec.samples.is_empty() {
                source.push(' ');
            }

            if !beat_spec.beat_no.fraction.is_zero() {
                source.push_str(&format!(
                    "{}:",
                    beat_spec
                        .beat_no
                        .fraction_name
                        .unwrap_or(&beat_spec.beat_no.fraction.to_string())
                ));
            }

            for sample_name in beat_spec.samples.iter() {
                source.push_str(&format!("{sample_name},"));
            }
            source.pop(); // Pop last trailing comma.

            source.push(' ');
            last_beat_no_integer = beat_spec.beat_no.beat.get();
        }
        source.pop(); // Pop last space
        source
    }
}

#[derive(Error, Debug, PartialEq)]
pub enum MeasureParseError {
    #[error("{0}")]
    SyntaxError(PestError<Rule>),
    #[error("logical error: {0}")]
    LogicalError(String),
}

pub static BEAT_FRACTIONS: &[(&str, BeatFraction)] = &[
    ("e", BeatFraction::ONE_FOURTH),
    ("&", BeatFraction::HALF),
    ("and", BeatFraction::HALF),
    ("ah", BeatFraction::THREE_FOURTHS),
    ("uh", BeatFraction::THREE_FOURTHS),
    ("a", BeatFraction::THREE_FOURTHS),
    ("trip", BeatFraction::ONE_THIRD),
    ("tri", BeatFraction::ONE_THIRD),
    ("let", BeatFraction::TWO_THIRDS),
    ("plet", BeatFraction::TWO_THIRDS),
];

static BEAT_FRACTION_MAP: LazyLock<HashMap<&str, BeatFraction>> =
    LazyLock::new(|| BEAT_FRACTIONS.iter().copied().collect());

fn resolve_beat_fraction_name(name: &str) -> (&'static str, BeatFraction) {
    let (&fraction_name, &fraction) = BEAT_FRACTION_MAP
        .get_key_value(name)
        .unwrap_or_else(|| panic!("beat fraction defined for name: {}", name));
    (fraction_name, fraction)
}

fn parse_measure_ast(measure_ast: Pair<Rule>) -> Result<Vec<BeatSpec>, String> {
    let mut beats = Vec::new();
    let mut position_str = "1";
    let mut beat_no = BeatNo::ONE;

    for measure_component in measure_ast.into_inner() {
        match measure_component.as_rule() {
            Rule::beat => {
                let mut samples = Vec::new();

                for beat_component in measure_component.into_inner() {
                    let mut new_position_str = position_str;
                    let mut new_position = beat_no;

                    match beat_component.as_rule() {
                        Rule::placement => {
                            let placement =
                                beat_component.into_inner().next().expect("beat placement");

                            new_position_str = placement.as_str();
                            new_position = match placement.as_rule() {
                                Rule::beat_no => {
                                    let no = new_position_str
                                        .parse::<NonZeroU32>()
                                        .expect("beat placement must be numeric");
                                    BeatNo::even_nonzero(no)
                                }
                                Rule::beat_fraction_name => {
                                    let (fraction_name, fraction) =
                                        resolve_beat_fraction_name(new_position_str);
                                    beat_no.with_named_fraction(fraction_name, fraction)
                                }
                                Rule::beat_fraction => {
                                    let (numerator_str, denominator_str) =
                                        new_position_str.split_once('/').expect("fraction string");
                                    let numerator: u32 =
                                        numerator_str.parse().expect("u32 beat numerator");
                                    let denominator: NonZeroU32 =
                                        denominator_str.parse().expect("non-zero beat denominator");

                                    let fraction =
                                        BeatFraction::new_nonzero(numerator, denominator).map_err(
                                            |e| {
                                                format!(
                                                    "beat fraction {} is invalid: error {e}",
                                                    backtick_quote(new_position_str)
                                                )
                                            },
                                        )?;

                                    beat_no.with_fraction(fraction)
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

                    if new_position < beat_no {
                        return Err(format!(
                            "beat number decreased: {} → {}",
                            backtick_quote(position_str),
                            backtick_quote(new_position_str)
                        ));
                    }

                    beat_no = new_position;
                    position_str = new_position_str;
                }

                beats.push(BeatSpec { samples, beat_no });
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

pub fn backtick_quote(k: &str) -> String {
    format!("`{}`", k.replace("`", "\\`"))
}

pub fn join_backtick_quoted<'a>(items: impl Iterator<Item = &'a str>) -> String {
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

pub fn parse_measure(
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
                beat_no: BeatNo::ONE,
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
                    beat_no: BeatNo::ONE,
                },
                BeatSpec {
                    samples: vec!["click".to_string()],
                    beat_no: BeatNo::even(2).unwrap(),
                },
                BeatSpec {
                    samples: vec!["click".to_string()],
                    beat_no: BeatNo::even(3).unwrap(),
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
                    beat_no: BeatNo::ONE,
                },
                BeatSpec {
                    samples: vec!["click".to_string()],
                    beat_no: BeatNo::even(2).unwrap(),
                },
                BeatSpec {
                    samples: vec![],
                    beat_no: BeatNo::even(3).unwrap(),
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
                    beat_no: BeatNo::ONE,
                },
                BeatSpec {
                    samples: vec![],
                    beat_no: BeatNo::even(2).unwrap(),
                },
                BeatSpec {
                    samples: vec!["click".to_string()],
                    beat_no: BeatNo::even(3).unwrap(),
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
                    beat_no: BeatNo::ONE,
                },
                BeatSpec {
                    samples: vec!["click".to_string()],
                    beat_no: BeatNo::new(1, 1, 2).unwrap(),
                },
                BeatSpec {
                    samples: vec!["CLICK".to_string()],
                    beat_no: BeatNo::even(2).unwrap(),
                },
                BeatSpec {
                    samples: vec!["click".to_string()],
                    beat_no: BeatNo::new(2, 1, 2).unwrap(),
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
                    beat_no: BeatNo::ONE,
                },
                BeatSpec {
                    samples: vec!["hat".to_string()],
                    beat_no: BeatNo::new(1, 1, 4).unwrap(),
                },
                BeatSpec {
                    samples: vec!["click".to_string()],
                    beat_no: BeatNo::new(1, 1, 2).unwrap(),
                },
                BeatSpec {
                    samples: vec!["hat".to_string()],
                    beat_no: BeatNo::new(1, 3, 4).unwrap(),
                },
                BeatSpec {
                    samples: vec!["CLICK".to_string()],
                    beat_no: BeatNo::even(2).unwrap(),
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
                    beat_no: BeatNo::ONE,
                },
                BeatSpec {
                    samples: vec!["hat".to_string()],
                    beat_no: BeatNo::new(1, 1, 3).unwrap(),
                },
                BeatSpec {
                    samples: vec!["click".to_string()],
                    beat_no: BeatNo::new(1, 2, 3).unwrap(),
                },
                BeatSpec {
                    samples: vec!["CLICK".to_string()],
                    beat_no: BeatNo::even(2).unwrap(),
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
                    beat_no: BeatNo::ONE,
                },
                BeatSpec {
                    samples: vec!["click".to_string()],
                    beat_no: BeatNo::new(1, 1, 5).unwrap(),
                },
                BeatSpec {
                    samples: vec!["click".to_string()],
                    beat_no: BeatNo::new(1, 2, 5).unwrap(),
                },
                BeatSpec {
                    samples: vec!["click".to_string()],
                    beat_no: BeatNo::new(1, 3, 5).unwrap(),
                },
                BeatSpec {
                    samples: vec!["click".to_string()],
                    beat_no: BeatNo::new(1, 4, 5).unwrap(),
                },
                BeatSpec {
                    samples: vec!["CLICK".to_string()],
                    beat_no: BeatNo::even(2).unwrap(),
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
                beat_no: BeatNo::ONE,
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
                    beat_no: BeatNo::ONE,
                },
                BeatSpec {
                    samples: vec!["hat".to_string(), "click".to_string()],
                    beat_no: BeatNo::new(1, 1, 3).unwrap(),
                },
                BeatSpec {
                    samples: vec!["click".to_string(), "hat".to_string()],
                    beat_no: BeatNo::new(1, 2, 3).unwrap(),
                },
                BeatSpec {
                    samples: vec!["CLICK".to_string()],
                    beat_no: BeatNo::even(2).unwrap(),
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
