use std::cmp::{Ordering, PartialEq, PartialOrd};
use std::fmt;
use std::ops::Add;

/// Played beat and beat fraction counter. Zero-based.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayedBeatCounter {
    beats: u64,
    fraction: f64,
}

impl PlayedBeatCounter {
    pub fn new(beats: u64, fraction: f64) -> Self {
        debug_assert!((0.0..1.0).contains(&fraction));
        Self { beats, fraction }
    }

    pub fn zero() -> Self {
        Self::new(0, 0.0)
    }

    pub fn beats(&self) -> u64 {
        self.beats
    }

    pub fn fraction(&self) -> f64 {
        self.fraction
    }

    pub fn advance_by(&mut self, beats: f64) {
        debug_assert!(beats >= 0.0);
        self.fraction += beats;
        self.beats += self.fraction.trunc() as u64;
        self.fraction = self.fraction.fract();
    }
}

impl PartialOrd for PlayedBeatCounter {
    fn partial_cmp(&self, other: &PlayedBeatCounter) -> Option<Ordering> {
        let beats_ord = self.beats.cmp(&other.beats);

        if beats_ord == Ordering::Equal {
            self.fraction.partial_cmp(&other.fraction)
        } else {
            Some(beats_ord)
        }
    }
}
impl Add<f64> for PlayedBeatCounter {
    type Output = PlayedBeatCounter;

    fn add(self, other: f64) -> Self::Output {
        let mut new = self;
        new.advance_by(other);
        new
    }
}

impl Add<u64> for PlayedBeatCounter {
    type Output = PlayedBeatCounter;

    fn add(self, other: u64) -> Self::Output {
        let mut new = self;
        new.beats += other;
        new
    }
}

impl fmt::Display for PlayedBeatCounter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        format!("{}+{}", self.beats, self.fraction).fmt(f)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use rstest::*;

    #[rstest]
    fn test_advance() {
        let mut counter = PlayedBeatCounter::zero();
        assert_eq!(counter.beats, 0);
        assert_eq!(counter.fraction, 0.0);

        counter.advance_by(1.0);
        assert_eq!(counter.beats, 1);
        assert_eq!(counter.fraction, 0.0);

        counter.advance_by(1.5);
        assert_eq!(counter.beats, 2);
        assert_eq!(counter.fraction, 0.5);

        counter.advance_by(1.5);
        assert_eq!(counter.beats, 4);
        assert_eq!(counter.fraction, 0.0);
    }

    #[rstest]
    #[case(11.12, 11, 0.12)]
    #[case(15029.91, 15029, 0.91)]
    fn test_advance_once(
        #[case] advanced_by: f64,
        #[case] expected_beats: u64,
        #[case] expected_fraction: f64,
    ) {
        let mut counter = PlayedBeatCounter::zero();

        counter.advance_by(advanced_by);
        assert_eq!(counter.beats, expected_beats);
        assert!((counter.fraction - expected_fraction).abs() < 1e-6);

        let mut counter = PlayedBeatCounter::zero();
        counter.advance_by(10.0);

        counter.advance_by(advanced_by);
        assert_eq!(counter.beats, expected_beats + 10);
        assert!((counter.fraction - expected_fraction).abs() < 1e-6);
    }

    #[rstest]
    #[case(
        PlayedBeatCounter::zero(),
        PlayedBeatCounter::zero(),
        Some(Ordering::Equal)
    )]
    #[case(
        PlayedBeatCounter::zero(),
        PlayedBeatCounter::new(1, 0.0),
        Some(Ordering::Less)
    )]
    #[case(
        PlayedBeatCounter::new(1, 0.0),
        PlayedBeatCounter::zero(),
        Some(Ordering::Greater)
    )]
    #[case(
        PlayedBeatCounter::new(0, 0.1),
        PlayedBeatCounter::new(0, 0.1),
        Some(Ordering::Equal)
    )]
    #[case(
        PlayedBeatCounter::new(0, 0.1),
        PlayedBeatCounter::new(0, 0.2),
        Some(Ordering::Less)
    )]
    #[case(
        PlayedBeatCounter::new(0, 0.2),
        PlayedBeatCounter::new(0, 0.1),
        Some(Ordering::Greater)
    )]
    #[case::beats_win_over_fraction_less(
        PlayedBeatCounter::new(1, 0.2),
        PlayedBeatCounter::new(2, 0.1),
        Some(Ordering::Less)
    )]
    #[case::beats_win_over_fraction_greater(
        PlayedBeatCounter::new(2, 0.1),
        PlayedBeatCounter::new(1, 0.2),
        Some(Ordering::Greater)
    )]
    #[case::fraction_partial_order_suppressed_by_beats(
        PlayedBeatCounter{beats: 1, fraction: f64::NAN},
        PlayedBeatCounter{beats: 2, fraction: f64::NAN},
        Some(Ordering::Less)
    )]
    #[case::fraction_partial_order_not_suppressed_by_beats(
        PlayedBeatCounter{beats: 1, fraction: f64::NAN},
        PlayedBeatCounter{beats: 1, fraction: f64::NAN},
        None
    )]
    fn test_partial_ord(
        #[case] l: PlayedBeatCounter,
        #[case] r: PlayedBeatCounter,
        #[case] ord: Option<Ordering>,
    ) {
        assert_eq!(l.partial_cmp(&r), ord);
    }
}
