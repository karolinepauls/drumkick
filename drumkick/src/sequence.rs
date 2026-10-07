use std::cmp::Ordering;
use std::fmt;
use std::num::NonZeroU32;
use thiserror::Error;

#[derive(Error, Copy, Clone, Debug)]
pub enum BeatError {
    #[error("beat numbers must start with one - got zero")]
    IllegalZeroBeatNo,
    #[error("beat fraction denominator cannot be zero")]
    IllegalZeroDenominator,
    #[error("beat fraction exceeds one")]
    FractionExceedsOne,
}

#[derive(Copy, Clone, Debug)]
pub struct BeatFraction {
    numerator: u32,
    denominator: NonZeroU32,
}

macro_rules! fraction {
    ($numerator:expr, $denominator:expr) => {{
        assert!($numerator < $denominator, "fraction must be less than one");
        Self {
            numerator: $numerator,
            denominator: NonZeroU32::new($denominator).unwrap(),
        }
    }};
}

impl BeatFraction {
    pub const ZERO: Self = fraction!(0, 1);
    pub const ONE_FOURTH: Self = fraction!(1, 4);
    pub const HALF: Self = fraction!(1, 2);
    pub const THREE_FOURTHS: Self = fraction!(3, 4);
    pub const ONE_THIRD: Self = fraction!(1, 3);
    pub const TWO_THIRDS: Self = fraction!(2, 3);

    pub fn new(numerator: u32, denominator: u32) -> Result<Self, BeatError> {
        if numerator >= denominator {
            return Err(BeatError::FractionExceedsOne);
        }
        Self::new_nonzero(
            numerator,
            NonZeroU32::new(denominator).ok_or(BeatError::IllegalZeroDenominator)?,
        )
    }

    pub const fn new_nonzero(numerator: u32, denominator: NonZeroU32) -> Result<Self, BeatError> {
        if numerator >= denominator.get() {
            return Err(BeatError::FractionExceedsOne);
        }
        Ok(Self {
            numerator,
            denominator,
        })
    }

    pub fn is_zero(&self) -> bool {
        self.numerator == 0
    }
}

impl fmt::Display for BeatFraction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "{}/{}", self.numerator, self.denominator)
    }
}

#[derive(Copy, Clone, Debug)]
pub struct BeatNo {
    beat: NonZeroU32,
    fraction: BeatFraction,
}

impl BeatNo {
    pub const ONE: Self = Self {
        beat: NonZeroU32::MIN,
        fraction: BeatFraction::ZERO,
    };

    pub fn new(beat: u32, numerator: u32, denominator: u32) -> Result<Self, BeatError> {
        if numerator >= denominator {
            return Err(BeatError::FractionExceedsOne);
        }
        Ok(Self {
            beat: NonZeroU32::new(beat).ok_or(BeatError::IllegalZeroBeatNo)?,
            fraction: BeatFraction::new(numerator, denominator)?,
        })
    }

    pub fn even_nonzero(beat: NonZeroU32) -> Self {
        Self {
            beat,
            fraction: BeatFraction::ZERO,
        }
    }

    pub fn even(beat: u32) -> Result<Self, BeatError> {
        Ok(Self {
            beat: NonZeroU32::new(beat).ok_or(BeatError::IllegalZeroBeatNo)?,
            fraction: BeatFraction::ZERO,
        })
    }

    pub fn with_fraction(&self, fraction: BeatFraction) -> Self {
        Self {
            beat: self.beat,
            fraction,
        }
    }

    pub fn beat(&self) -> NonZeroU32 {
        self.beat
    }

    pub fn fraction(&self) -> BeatFraction {
        self.fraction
    }

    pub fn as_position_nanos(&self) -> u64 {
        (self.as_position() * 1e9).round() as u64
    }

    pub fn as_position(&self) -> f64 {
        let seconds = f64::from(self.beat.get() - 1);
        let fraction =
            f64::from(self.fraction.numerator) / f64::from(self.fraction.denominator.get());
        seconds + fraction
    }
}

impl PartialEq for BeatNo {
    fn eq(&self, other: &Self) -> bool {
        self.as_position().eq(&other.as_position())
    }
}

impl Eq for BeatNo {}

impl PartialOrd for BeatNo {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for BeatNo {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_position_nanos().cmp(&other.as_position_nanos())
    }
}

impl fmt::Display for BeatNo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        write!(f, "{}", self.beat)?;
        if !self.fraction.is_zero() {
            write!(f, " ")?;
            self.fraction.fmt(f)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use rstest::*;

    const BILLION: u64 = 1_000_000_000;

    fn one_and_two_fourts() -> BeatNo {
        BeatNo::new(1, 2, 4).unwrap()
    }

    fn one_and_one_second() -> BeatNo {
        BeatNo::new(1, 1, 2).unwrap()
    }

    #[rstest]
    #[case(BeatNo::ONE, "1")]
    #[case(one_and_one_second(), "1 1/2")]
    #[case(one_and_two_fourts(), "1 2/4")]
    fn test_beat_to_string(#[case] beat_no: BeatNo, #[case] expected: &str) {
        assert_eq!(beat_no.to_string(), expected);
    }

    #[rustfmt::skip]
    #[rstest]
    #[case(
        BeatNo::ONE,
        0.0, 0,
    )]
    #[case(
        BeatNo::even(2).unwrap(), 1.0,
        1 * BILLION
    )]
    #[case(
        BeatNo::even(100).unwrap(), 99.0,
        99 * BILLION
    )]
    #[case(
        BeatNo::new(1, 1, 2).unwrap(),
        0.5,
        BILLION / 2
    )]
    #[case(
        BeatNo::new(1, 1, 3).unwrap(),
        1.0 / 3.0,
        BILLION / 3
    )]
    #[case(
        BeatNo::new(1, 2, 3).unwrap(),
        2.0 / 3.0,
        2 * BILLION / 3 + 1  // Rounded up
    )]
    #[case(
        BeatNo::new(4, 1, 3).unwrap(),
        3.0 + 1.0 / 3.0,
        BILLION * 3 + BILLION / 3
    )]
    fn test_beat_no_position(
        #[case] beat_no: BeatNo,
        #[case] position: f64,
        #[case] position_nanos: u64,
    ) {
        assert_eq!(beat_no.as_position(), position);
        assert_eq!(beat_no.as_position_nanos(), position_nanos);
    }

    #[rstest]
    #[case(BeatNo::ONE, BeatNo::ONE)]
    #[case(
        BeatNo::new(1, 2, 5).unwrap(),
        BeatNo::new(1, 2, 5).unwrap(),
    )]
    #[case(one_and_one_second(), one_and_two_fourts())]
    #[case(one_and_two_fourts(), one_and_two_fourts())]
    fn test_beats_eq(#[case] left: BeatNo, #[case] right: BeatNo) {
        assert_eq!(left, right);
    }

    #[rstest]
    #[case(BeatNo::ONE, BeatNo::even(2).unwrap())]
    #[case(
        BeatNo::new(1, 3, 4).unwrap(),
        BeatNo::new(1, 2, 4).unwrap(),
    )]
    fn test_beats_not_eq(#[case] left: BeatNo, #[case] right: BeatNo) {
        assert_ne!(left, right);
    }

    #[rstest]
    #[case(
        &[
            BeatNo::even(2).unwrap(),
            BeatNo::even(3).unwrap(),
            BeatNo::even(1).unwrap(),
        ],
        &[
            BeatNo::even(1).unwrap(),
            BeatNo::even(2).unwrap(),
            BeatNo::even(3).unwrap(),
        ]
    )]
    #[case(
        &[
            BeatNo::new(1, 2, 6).unwrap(),
            BeatNo::new(2, 1, 3).unwrap(),
            BeatNo::new(3, 1, 3).unwrap(),
        ],
        &[
            BeatNo::new(1, 1, 3).unwrap(),
            BeatNo::new(2, 1, 3).unwrap(),
            BeatNo::new(3, 1, 3).unwrap(),
        ]
    )]
    #[case(
        &[
            BeatNo::new(4, 1, 3).unwrap(),
            BeatNo::new(1, 2, 3).unwrap(),
            BeatNo::new(1, 4, 12).unwrap(),
            BeatNo::new(2, 2, 3).unwrap(),
        ],
        &[
            BeatNo::new(1, 4, 12).unwrap(),
            BeatNo::new(1, 2, 3).unwrap(),
            BeatNo::new(2, 2, 3).unwrap(),
            BeatNo::new(4, 1, 3).unwrap(),
        ]
    )]
    fn test_beats_sorted(#[case] unsorted: &[BeatNo], #[case] expected: &[BeatNo]) {
        let mut sorted: Vec<_> = unsorted.into();
        sorted.sort();
        assert_eq!(sorted, expected);
    }

    #[rstest]
    #[case::equivalent_stable(
        &[
            one_and_one_second(),
            one_and_two_fourts(),
        ],
        &[
            one_and_one_second(),
            one_and_two_fourts(),
        ]
    )]
    #[case::equivalent_stable_reverse(
        &[
            one_and_two_fourts(),
            one_and_one_second(),
        ],
        &[
            one_and_two_fourts(),
            one_and_one_second(),
        ]
    )]
    fn test_beats_sorted_stable(#[case] unsorted: &[BeatNo], #[case] expected: &[BeatNo]) {
        let mut sorted: Vec<_> = unsorted.into();
        sorted.sort();
        assert_eq!(
            sorted
                .into_iter()
                .map(|b| b.to_string())
                .collect::<Vec<_>>(),
            expected
                .into_iter()
                .map(|b| b.to_string())
                .collect::<Vec<_>>(),
        );
    }
}
